//! RTF/CRE steno dictionary import.
//!
//! Commercial dictionaries ship as RTF/CRE, an RTF document whose steno
//! specific control words (`\cxs`, `\cxp`, `\cxfing`, `\cxds`, ...) carry the
//! outline and the formatting intent. This module reads one and hands back
//! `(outline, translation)` pairs whose translations use exactly the meta
//! command syntax [`crate::format`] already understands, so an imported
//! dictionary behaves like a JSON one.
//!
//! Spec, for the control words below:
//! <https://web.archive.org/web/20201017075356/http://www.legalxml.org/workgroups/substantive/transcripts/cre-spec.htm>
//!
//! RTF is a byte format, not UTF-8. [`load`] decodes code page 1252, which is
//! what the `\ansi` charset every dictionary in the wild declares means, and
//! `\'xx` escapes are decoded through the same table. The five byte values
//! cp1252 leaves undefined are reported as errors rather than substituted:
//! a replacement character in a dictionary entry is a silent wrong answer
//! every time that outline is written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum RtfError {
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("line {line}: byte 0x{byte:02x} has no character in code page 1252")]
    Encoding { line: usize, byte: u8 },
    #[error("line {line}: malformed escape: {detail}")]
    Escape { line: usize, detail: String },
    #[error(r"not an RTF/CRE dictionary: expected it to open with {{\rtf1")]
    BadHeader,
    #[error("unexpected end of file with {open} group(s) still open")]
    UnexpectedEof { open: usize },
    #[error("line {line}: content after the group that closes the document")]
    TrailingContent { line: usize },
    #[error(r"line {line}: a new \cxs entry began with {open} group(s) still open")]
    UnfinishedEntry { line: usize, open: usize },
    #[error(r"line {line}: \{control} expects text to follow it")]
    ExpectedText { line: usize, control: String },
}

/// Parse an RTF/CRE dictionary into (outline, translation) pairs, where the
/// translation uses Pluvialis meta command syntax ({^suffix}, {-|}, etc).
pub fn parse(source: &str) -> Result<Vec<(String, String)>, RtfError> {
    let chars: Vec<char> = source.chars().collect();
    let tokens = tokenize(&chars)?;
    parse_tokens(&chars, &tokens)
}

/// Read and parse an RTF/CRE dictionary file.
pub fn load(path: impl AsRef<Path>) -> Result<Vec<(String, String)>, RtfError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| RtfError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let source = decode_cp1252(&bytes)?;
    parse(&source)
}

// ---------------------------------------------------------------------------
// Code page 1252
// ---------------------------------------------------------------------------

/// Characters for bytes 0x80 to 0x9F. Zero marks the five values cp1252 leaves
/// undefined; every other byte maps to the Latin-1 code point of the same
/// value.
const CP1252_HIGH: [u32; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
    0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

/// The character a byte stands for in code page 1252, or `None` for the five
/// undefined values.
fn cp1252_char(byte: u8) -> Option<char> {
    match byte {
        0x80..=0x9F => match CP1252_HIGH[(byte - 0x80) as usize] {
            // The sentinel, not U+0000: these five byte values are undefined.
            0 => None,
            value => char::from_u32(value),
        },
        other => Some(other as char),
    }
}

fn decode_cp1252(bytes: &[u8]) -> Result<String, RtfError> {
    let mut out = String::with_capacity(bytes.len());
    let mut line = 1usize;
    for &byte in bytes {
        match cp1252_char(byte) {
            Some(c) => {
                if c == '\n' {
                    line += 1;
                }
                out.push(c);
            }
            None => return Err(RtfError::Encoding { line, byte }),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokKind {
    GroupStart,
    GroupEnd,
    /// A control word or control symbol without its leading backslash. Any
    /// numeric parameter stays attached, so `\rtf1` is `rtf1` and the style
    /// `\s12` is `s12`, which is what the style test below needs.
    Control(String),
    Text(String),
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TokKind,
    /// Character offset, used only to report a line number on error.
    at: usize,
}

fn line_of(chars: &[char], at: usize) -> usize {
    1 + chars[..at.min(chars.len())]
        .iter()
        .filter(|&&c| c == '\n')
        .count()
}

fn push_text(out: &mut Vec<Tok>, pending: &mut String, at: usize) {
    if !pending.is_empty() {
        out.push(Tok {
            kind: TokKind::Text(std::mem::take(pending)),
            at,
        });
    }
}

/// Split RTF into groups, controls and text runs.
///
/// `\'xx` and `\uNNNN` are resolved here and merged into the surrounding text
/// run, because the parser's `\cxds` rules key off whole text runs: splitting
/// `caf\'e9` into two tokens would turn an infix into a prefix.
fn tokenize(chars: &[char]) -> Result<Vec<Tok>, RtfError> {
    let mut out: Vec<Tok> = Vec::new();
    let mut pending = String::new();
    let mut pending_at = 0usize;
    // \ucN: how many fallback characters follow each \uNNNN. Tracked globally
    // rather than per group; dictionaries set it once in the header if at all.
    let mut uc = 1usize;
    let mut high_surrogate: Option<u16> = None;
    let mut i = 0usize;

    while i < chars.len() {
        match chars[i] {
            '{' | '}' => {
                push_text(&mut out, &mut pending, pending_at);
                let kind = if chars[i] == '{' {
                    TokKind::GroupStart
                } else {
                    TokKind::GroupEnd
                };
                out.push(Tok { kind, at: i });
                i += 1;
            }
            // Raw line breaks are layout in the source file, not content.
            '\r' | '\n' => i += 1,
            '\\' => {
                let start = i;
                i += 1;
                let Some(&c) = chars.get(i) else {
                    return Err(RtfError::Escape {
                        line: line_of(chars, start),
                        detail: "file ends with a lone backslash".to_owned(),
                    });
                };
                if c == '\'' {
                    let byte = read_hex_byte(chars, i + 1, start)?;
                    let ch = cp1252_char(byte).ok_or(RtfError::Encoding {
                        line: line_of(chars, start),
                        byte,
                    })?;
                    if pending.is_empty() {
                        pending_at = start;
                    }
                    pending.push(ch);
                    i += 3;
                } else if c.is_ascii_alphabetic() {
                    let (word, param, next) = read_control_word(chars, i);
                    i = next;
                    match word.as_str() {
                        "u" => {
                            let Some(param) = param else {
                                return Err(RtfError::Escape {
                                    line: line_of(chars, start),
                                    detail: r"\u without a code point".to_owned(),
                                });
                            };
                            let value = if param < 0 { param + 0x1_0000 } else { param };
                            let Ok(unit) = u16::try_from(value) else {
                                return Err(RtfError::Escape {
                                    line: line_of(chars, start),
                                    detail: format!(r"\u{param} is out of range"),
                                });
                            };
                            if let Some(ch) =
                                combine_unicode(unit, &mut high_surrogate, chars, start)?
                            {
                                if pending.is_empty() {
                                    pending_at = start;
                                }
                                pending.push(ch);
                            }
                            skip_fallback(chars, &mut i, uc);
                        }
                        "uc" => {
                            if let Some(param) = param
                                && let Ok(count) = usize::try_from(param)
                            {
                                uc = count;
                            }
                        }
                        _ => {
                            push_text(&mut out, &mut pending, pending_at);
                            let mut token = word;
                            if let Some(param) = param {
                                token.push_str(&param.to_string());
                            }
                            out.push(Tok {
                                kind: TokKind::Control(token),
                                at: start,
                            });
                        }
                    }
                } else {
                    // Control symbol: always exactly one character. An escaped
                    // line break carries the same meaning as \par.
                    push_text(&mut out, &mut pending, pending_at);
                    let name = match c {
                        '\r' | '\n' => "\n".to_owned(),
                        other => other.to_string(),
                    };
                    out.push(Tok {
                        kind: TokKind::Control(name),
                        at: start,
                    });
                    i += 1;
                }
            }
            c => {
                if pending.is_empty() {
                    pending_at = i;
                }
                pending.push(c);
                i += 1;
            }
        }
    }

    if high_surrogate.is_some() {
        return Err(RtfError::Escape {
            line: line_of(chars, chars.len()),
            detail: r"\u surrogate pair is missing its second half".to_owned(),
        });
    }
    push_text(&mut out, &mut pending, pending_at);
    Ok(out)
}

fn read_hex_byte(chars: &[char], at: usize, start: usize) -> Result<u8, RtfError> {
    let bad = || RtfError::Escape {
        line: line_of(chars, start),
        detail: r"\' needs two hex digits".to_owned(),
    };
    let hi = chars.get(at).copied().ok_or_else(bad)?;
    let lo = chars.get(at + 1).copied().ok_or_else(bad)?;
    let hi = hi.to_digit(16).ok_or_else(bad)?;
    let lo = lo.to_digit(16).ok_or_else(bad)?;
    Ok((hi * 16 + lo) as u8)
}

/// Read the letters, optional signed parameter, and the single optional space
/// that delimits a control word. `at` points at the first letter.
fn read_control_word(chars: &[char], at: usize) -> (String, Option<i32>, usize) {
    let mut i = at;
    let mut word = String::new();
    while let Some(&c) = chars.get(i) {
        if !c.is_ascii_alphabetic() {
            break;
        }
        word.push(c);
        i += 1;
    }

    let mut digits = String::new();
    if chars.get(i) == Some(&'-') {
        digits.push('-');
        i += 1;
    }
    while let Some(&c) = chars.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        digits.push(c);
        i += 1;
    }
    // A lone '-' with no digits is not a parameter, so give the character back.
    if digits == "-" {
        digits.clear();
        i -= 1;
    }
    let param = digits.parse::<i32>().ok();

    // Exactly one space is the delimiter and is not text.
    if chars.get(i) == Some(&' ') {
        i += 1;
    }
    (word, param, i)
}

/// Turn a `\u` code unit into a character, pairing surrogates across two
/// escapes. Returns `Ok(None)` when the unit was a high surrogate being held.
fn combine_unicode(
    unit: u16,
    high: &mut Option<u16>,
    chars: &[char],
    at: usize,
) -> Result<Option<char>, RtfError> {
    let bad = |detail: &str| RtfError::Escape {
        line: line_of(chars, at),
        detail: detail.to_owned(),
    };

    if let Some(lead) = high.take() {
        if !(0xDC00..=0xDFFF).contains(&unit) {
            return Err(bad(r"\u surrogate pair is missing its second half"));
        }
        let value = 0x1_0000 + ((u32::from(lead) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
        return char::from_u32(value)
            .map(Some)
            .ok_or_else(|| bad(r"\u surrogate pair is not a character"));
    }
    if (0xD800..=0xDBFF).contains(&unit) {
        *high = Some(unit);
        return Ok(None);
    }
    char::from_u32(u32::from(unit))
        .map(Some)
        .ok_or_else(|| bad(r"\u value is not a character"))
}

/// Step over the ASCII fallback that follows `\uNNNN` for readers that cannot
/// show the real character. Stops at a group boundary so a malformed count
/// cannot eat structure.
fn skip_fallback(chars: &[char], i: &mut usize, count: usize) {
    let mut done = 0usize;
    while done < count {
        match chars.get(*i) {
            None | Some('{') | Some('}') => break,
            Some('\\') => {
                if chars.get(*i + 1) == Some(&'\'') {
                    *i = (*i + 4).min(chars.len());
                    done += 1;
                } else {
                    break;
                }
            }
            Some(_) => {
                *i += 1;
                done += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// `\s0`, `\s12`: a style reference, as opposed to a steno control word.
fn is_style(word: &str) -> bool {
    match word.strip_prefix('s') {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// Braces are meta command delimiters to [`crate::format`], so a literal brace
/// coming out of RTF has to be escaped or it silently opens a meta.
fn escape_braces(s: &str) -> String {
    if !s.contains(['{', '}']) {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if c == '{' || c == '}' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `\cxp` holds the punctuation itself rather than a code, so it is classified
/// by what it is: sentence enders and clause marks get the punctuation metas,
/// everything else attaches on both sides so it cannot gain a stray space.
fn punctuation_meta(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed {
        "." | "!" | "?" | "," | ";" | ":" => format!("{{{trimmed}}}"),
        "'" => "{^'}".to_owned(),
        "-" | "/" => format!("{{^{trimmed}^}}"),
        _ => format!("{{^{raw}^}}"),
    }
}

/// Rescue two shapes that dictionaries express as bare text.
///
/// caseCATalyst does not wrap punctuation in `\cxp`, so a translation that is
/// nothing but a punctuation mark has to be recognised here or it would arrive
/// with a leading space. Runs of two or more spaces at either end are
/// deliberate spacing and become attach metas, since a single trailing space
/// would otherwise be eaten by the formatter's own spacing.
fn finalize_translation(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut chars = text.chars();
    if let Some(first) = chars.next() {
        let rest = chars.as_str();
        if ".?!:;,".contains(first) && (rest.is_empty() || rest == " ") {
            return format!("{{{first}}}{rest}");
        }
    }

    let mut out = text.to_owned();
    let lead = out.len() - out.trim_start().len();
    if out[..lead].chars().count() > 1 {
        out = format!("{{^{}^}}{}", &out[..lead], &out[lead..]);
    }
    let trail = out.len() - out.trim_end().len();
    if trail > 0 {
        let cut = out.len() - trail;
        if out[cut..].chars().count() > 1 {
            out = format!("{}{{^{}^}}", &out[..cut], &out[cut..]);
        }
    }
    out
}

/// A control word that maps to a fixed piece of output.
fn simple_control(word: &str) -> Option<&'static str> {
    Some(match word {
        // A stray \* outside a group start marks nothing we act on.
        "*" => "",
        // Hard space and non-breaking hyphen: both attach on both sides.
        "~" => "{^ ^}",
        "_" => "{^-^}",
        // An escaped line break, and \ followed by a space, both mean \par.
        "" | "\n" => "\n\n",
        "\\" => "\\",
        "{" => r"\{",
        "}" => r"\}",
        "-" => "-",
        "line" => "\n",
        "par" => "\n\n",
        "tab" => "\t",
        // Force cap and force lower case.
        "cxfc" => "{-|}",
        "cxfl" => "{>}",
        _ => return None,
    })
}

/// Destinations we handle ourselves even when the file marks them ignorable
/// with `\*`. Everything else marked ignorable really is skipped.
fn is_known_destination(word: &str) -> bool {
    matches!(
        word,
        "cxfing" | "cxstit" | "cxsvatdictflags" | "cxplovermacro" | "cxplovermeta"
    )
}

struct Cursor<'a> {
    tokens: &'a [Tok],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn advance(&mut self) -> Option<&'a Tok> {
        let tok = self.tokens.get(self.pos);
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn peek(&self) -> Option<&'a Tok> {
        self.tokens.get(self.pos)
    }

    fn rewind(&mut self) {
        debug_assert!(self.pos > 0);
        self.pos -= 1;
    }

    fn is_control(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Tok { kind: TokKind::Control(w), .. }) if w == word)
    }
}

fn parse_tokens(chars: &[char], tokens: &[Tok]) -> Result<Vec<(String, String)>, RtfError> {
    let mut cur = Cursor { tokens, pos: 0 };

    let header_ok = matches!(
        (cur.advance().map(|t| &t.kind), cur.advance().map(|t| &t.kind)),
        (Some(TokKind::GroupStart), Some(TokKind::Control(w))) if w == "rtf1"
    );
    if !header_ok {
        return Err(RtfError::BadHeader);
    }

    // The document group itself is not on the stack; `dest`/`text` are its
    // state, and the stack holds the groups opened inside it.
    let mut dest: Option<String> = Some("rtf1".to_owned());
    let mut text = String::new();
    let mut stack: Vec<(Option<String>, String)> = Vec::new();
    let mut stylesheet: HashMap<String, String> = HashMap::new();
    let mut steno: Option<String> = None;
    let mut entries: Vec<(String, String)> = Vec::new();

    loop {
        let Some(tok) = cur.advance() else {
            return Err(RtfError::UnexpectedEof {
                open: stack.len() + 1,
            });
        };
        let line = || line_of(chars, tok.at);

        match &tok.kind {
            TokKind::GroupStart => {
                let mut ignored = false;
                let mut rewind = false;
                let mut destination: Option<String> = None;

                let Some(mut inner) = cur.advance() else {
                    return Err(RtfError::UnexpectedEof {
                        open: stack.len() + 2,
                    });
                };
                if matches!(&inner.kind, TokKind::Control(w) if w == "*") {
                    ignored = true;
                    let Some(next) = cur.advance() else {
                        return Err(RtfError::UnexpectedEof {
                            open: stack.len() + 2,
                        });
                    };
                    inner = next;
                }

                if let TokKind::Control(word) = &inner.kind {
                    destination = Some(word.clone());
                    if word == "cxs" {
                        if !stack.is_empty() {
                            return Err(RtfError::UnfinishedEntry {
                                line: line_of(chars, inner.at),
                                open: stack.len(),
                            });
                        }
                        // The previous entry's translation is whatever
                        // accumulated since its own \cxs group closed.
                        if let Some(outline) = steno.take() {
                            entries.push((outline, finalize_translation(&text)));
                        }
                        ignored = false;
                        text.clear();
                    } else if is_known_destination(word) {
                        ignored = false;
                    } else if !is_style(word) {
                        // Not a destination we know: the control word still has
                        // to run, inside the group it just opened.
                        rewind = true;
                    }
                } else {
                    rewind = true;
                }

                if ignored {
                    skip_group(&mut cur, stack.len())?;
                    continue;
                }

                stack.push((dest.take(), std::mem::take(&mut text)));
                dest = destination;
                if rewind {
                    cur.rewind();
                }
            }

            TokKind::GroupEnd => {
                let Some((parent_dest, parent_text)) = stack.pop() else {
                    // The document group just closed; nothing may follow.
                    return match cur.advance() {
                        None => {
                            if let Some(outline) = steno.take() {
                                entries.push((outline, finalize_translation(&text)));
                            }
                            Ok(entries)
                        }
                        Some(extra) => Err(RtfError::TrailingContent {
                            line: line_of(chars, extra.at),
                        }),
                    };
                };

                let contribution = match dest.as_deref() {
                    Some("cxs") => {
                        steno = Some(text.clone());
                        String::new()
                    }
                    Some("cxp") => punctuation_meta(&text),
                    Some("cxsvatdictflags") => {
                        // Stenovations flags: N means the entry forces a capital.
                        if text.contains('N') {
                            "{-|}".to_owned()
                        } else {
                            String::new()
                        }
                    }
                    Some("cxfing") => format!("{{&{text}}}"),
                    Some("cxstit") => format!("{{:stitch:{text}}}"),
                    Some("cxplovermacro") => format!("={text}"),
                    Some("cxplovermeta") => format!("{{{text}}}"),
                    Some(style)
                        if is_style(style) && parent_dest.as_deref() == Some("stylesheet") =>
                    {
                        stylesheet.insert(style.to_owned(), text.clone());
                        String::new()
                    }
                    _ => text.clone(),
                };

                dest = parent_dest;
                text = parent_text;
                text.push_str(&contribution);
            }

            TokKind::Control(word) => {
                if let Some(fixed) = simple_control(word) {
                    text.push_str(fixed);
                    continue;
                }
                match word.as_str() {
                    // Delete space. What follows decides whether this is a
                    // prefix, an infix, or a bare attach.
                    "cxds" => {
                        if let Some(Tok {
                            kind: TokKind::Text(body),
                            ..
                        }) = cur.peek()
                        {
                            let body = body.clone();
                            cur.advance();
                            if cur.is_control("cxds") {
                                cur.advance();
                                text.push_str(&format!("{{^{}^}}", escape_braces(&body)));
                            } else {
                                text.push_str(&format!("{{^{}}}", escape_braces(&body)));
                            }
                        } else {
                            text.push_str("{^}");
                        }
                    }
                    // Delete last stroke: the whole translation is the command.
                    "cxdstroke" => text = "=undo".to_owned(),
                    "cxfing" | "cxstit" => {
                        let Some(Tok {
                            kind: TokKind::Text(body),
                            ..
                        }) = cur.peek()
                        else {
                            return Err(RtfError::ExpectedText {
                                line: line(),
                                control: word.clone(),
                            });
                        };
                        let body = escape_braces(body);
                        cur.advance();
                        if word == "cxfing" {
                            text.push_str(&format!("{{&{body}}}"));
                        } else {
                            text.push_str(&format!("{{:stitch:{body}}}"));
                        }
                    }
                    style if is_style(style) => {
                        // caseCATalyst switches style without a preceding \par.
                        if !text.ends_with("\n\n") {
                            text.push_str("\n\n");
                        }
                        if stylesheet
                            .get(style)
                            .is_some_and(|name| name.starts_with("Contin"))
                        {
                            text.push_str("    ");
                        }
                    }
                    // Any other control word is presentation, not content.
                    _ => {}
                }
            }

            TokKind::Text(body) => {
                let mut body = escape_braces(body);
                if cur.is_control("cxds") {
                    cur.advance();
                    body = format!("{{{body}^}}");
                }
                text.push_str(&body);
            }
        }
    }
}

/// Consume an ignorable group, which has already had its `{` read.
fn skip_group(cur: &mut Cursor, open_below: usize) -> Result<(), RtfError> {
    let mut depth = 1usize;
    while depth > 0 {
        let Some(tok) = cur.advance() else {
            return Err(RtfError::UnexpectedEof {
                open: open_below + depth + 1,
            });
        };
        match tok.kind {
            TokKind::GroupStart => depth += 1,
            TokKind::GroupEnd => depth -= 1,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap entry bodies in the header every RTF/CRE file carries.
    fn doc(body: &str) -> String {
        format!(r"{{\rtf1\ansi\cxdict{body}}}")
    }

    fn entries(body: &str) -> Vec<(String, String)> {
        parse(&doc(body)).expect("dictionary parses")
    }

    fn one(body: &str) -> (String, String) {
        let mut got = entries(body);
        assert_eq!(got.len(), 1, "expected exactly one entry, got {got:?}");
        got.pop().expect("checked length")
    }

    #[test]
    fn a_minimal_dictionary_parses_to_its_pairs() {
        assert_eq!(
            entries(r"{\*\cxs KAT}cat{\*\cxs TKOG}dog"),
            vec![
                ("KAT".to_owned(), "cat".to_owned()),
                ("TKOG".to_owned(), "dog".to_owned()),
            ]
        );
    }

    #[test]
    fn an_empty_dictionary_yields_no_entries() {
        assert_eq!(entries(""), Vec::new());
        assert_eq!(parse(r"{\rtf1}").unwrap(), Vec::new());
    }

    #[test]
    fn multi_stroke_outlines_survive_intact() {
        assert_eq!(
            one(r"{\*\cxs WEL/KO*PL}welcome"),
            ("WEL/KO*PL".to_owned(), "welcome".to_owned())
        );
    }

    #[test]
    fn the_last_entry_is_emitted_at_the_closing_brace() {
        // Regression guard: the final entry has no following \cxs to flush it.
        let got = entries(r"{\*\cxs A}first{\*\cxs B}last");
        assert_eq!(got.last().unwrap().1, "last");
    }

    #[test]
    fn punctuation_groups_become_punctuation_metas() {
        assert_eq!(one(r"{\*\cxs TP-PL}{\cxp. }").1, "{.}");
        assert_eq!(one(r"{\*\cxs KW-BG}{\cxp, }").1, "{,}");
        assert_eq!(one(r"{\*\cxs KW-PL}{\cxp? }").1, "{?}");
        assert_eq!(one(r"{\*\cxs STPH-FPLT}{\cxp; }").1, "{;}");
        // An apostrophe attaches only on the left, so a possessive works.
        assert_eq!(one(r"{\*\cxs AE}{\cxp' }").1, "{^'}");
        // A dash attaches on both sides.
        assert_eq!(one(r"{\*\cxs H-PB}{\cxp- }").1, "{^-^}");
        // Anything unrecognised is passed through verbatim, spacing and all.
        assert_eq!(one(r"{\*\cxs STPH}{\cxp << }").1, "{^<< ^}");
    }

    #[test]
    fn a_bare_punctuation_translation_is_recognised_without_cxp() {
        // caseCATalyst writes the mark as plain text.
        assert_eq!(one(r"{\*\cxs TP-PL}.").1, "{.}");
        assert_eq!(one(r"{\*\cxs TP-PL}. ").1, "{.} ");
        // A word starting with a mark is not punctuation.
        assert_eq!(one(r"{\*\cxs TK-T}.com").1, ".com");
    }

    #[test]
    fn fingerspelling_becomes_a_glue_meta() {
        assert_eq!(one(r"{\*\cxs A*}{\cxfing a}").1, "{&a}");
        // The ignorable marker on the group must not skip it.
        assert_eq!(one(r"{\*\cxs A*}{\*\cxfing a}").1, "{&a}");
        // Control word form, no group.
        assert_eq!(one(r"{\*\cxs P*}\cxfing p").1, "{&p}");
    }

    #[test]
    fn stitching_becomes_a_stitch_meta() {
        assert_eq!(one(r"{\*\cxs TP*}{\cxstit F}").1, "{:stitch:F}");
        assert_eq!(one(r"{\*\cxs PW*}\cxstit B").1, "{:stitch:B}");
    }

    #[test]
    fn delete_space_makes_prefixes_infixes_and_suffixes() {
        // Prefix: \cxds then text.
        assert_eq!(one(r"{\*\cxs KR-}\cxds con").1, "{^con}");
        // Infix: \cxds on both sides.
        assert_eq!(one(r"{\*\cxs H-F}\cxds of \cxds").1, "{^of ^}");
        // Suffix: text then \cxds.
        assert_eq!(one(r"{\*\cxs -G}ing\cxds ").1, "{ing^}");
        // Bare: nothing but the space suppression.
        assert_eq!(one(r"{\*\cxs TK-LS}\cxds").1, "{^}");
    }

    #[test]
    fn bare_delete_space_before_a_group_does_not_swallow_it() {
        // \cxds is followed by a group, not text, so it stays a bare attach.
        assert_eq!(one(r"{\*\cxs TK-LS}\cxds{\cxp. }").1, "{^}{.}");
    }

    #[test]
    fn capitalisation_controls_map_to_case_metas() {
        assert_eq!(one(r"{\*\cxs KPA}\cxfc ").1, "{-|}");
        assert_eq!(one(r"{\*\cxs HRO*ER}\cxfl ").1, "{>}");
        assert_eq!(one(r"{\*\cxs KPA*}\cxfc word").1, "{-|}word");
    }

    #[test]
    fn stenovations_dict_flags_force_a_capital() {
        assert_eq!(
            one(r"{\*\cxs TPHAPL}{\*\cxsvatdictflags N}name").1,
            "{-|}name"
        );
        // Flags without N contribute nothing.
        assert_eq!(one(r"{\*\cxs TPHAPL}{\*\cxsvatdictflags X}name").1, "name");
    }

    #[test]
    fn hard_space_and_non_breaking_hyphen() {
        assert_eq!(one(r"{\*\cxs SP-S}\~").1, "{^ ^}");
        assert_eq!(one(r"{\*\cxs H-PB}\_").1, "{^-^}");
    }

    #[test]
    fn breaks_and_tabs_become_literal_whitespace() {
        // A translation that is nothing but a paragraph break is edge
        // whitespace, so it comes back wrapped rather than losing its spacing.
        assert_eq!(one(r"{\*\cxs R-R}\par ").1, "{^\n\n^}");
        assert_eq!(one(r"{\*\cxs R-R}\line ").1, "\n");
        assert_eq!(one(r"{\*\cxs TA-B}\tab ").1, "\t");
        assert_eq!(one(r"{\*\cxs R-R}x\par y").1, "x\n\ny");
    }

    #[test]
    fn plover_meta_and_macro_groups_pass_through() {
        assert_eq!(
            one(r"{\*\cxs PHOD}{\*\cxplovermeta MODE:CAPS}").1,
            "{MODE:CAPS}"
        );
        assert_eq!(
            one(r"{\*\cxs TPHO}{\*\cxplovermacro retrospective_delete_space}").1,
            "=retrospective_delete_space"
        );
        assert_eq!(one(r"{\*\cxs TK-LS}\cxdstroke").1, "=undo");
    }

    #[test]
    fn nested_groups_contribute_their_text_in_order() {
        assert_eq!(one(r"{\*\cxs KAT}{a{b{c}d}e}f").1, "abcdef");
        // A group opened by an unknown control word still runs that control.
        assert_eq!(one(r"{\*\cxs KAT}{\b bold}text").1, "boldtext");
    }

    #[test]
    fn ignorable_groups_are_skipped_including_their_children() {
        let got = entries(r"{\*\cxrev100}{\*\cxsystem Some CAT{\*\nested x}}{\*\cxs KAT}cat");
        assert_eq!(got, vec![("KAT".to_owned(), "cat".to_owned())]);
        // And one sitting inside an entry contributes nothing.
        assert_eq!(one(r"{\*\cxs KAT}ca{\*\cxcomment ignore me}t").1, "cat");
    }

    #[test]
    fn a_stylesheet_is_read_without_leaking_into_translations() {
        let got = entries(r"{\stylesheet{\s0 Normal;}{\s1 Continuation;}}{\*\cxs KAT}cat");
        assert_eq!(got, vec![("KAT".to_owned(), "cat".to_owned())]);
    }

    #[test]
    fn a_continuation_style_indents_and_others_only_break() {
        let got = one(r"{\stylesheet{\s1 Continuation;}}{\*\cxs KAT}\s1 cat");
        assert_eq!(got.1, "{^\n\n    ^}cat");
        let plain = one(r"{\stylesheet{\s0 Normal;}}{\*\cxs KAT}\s0 cat");
        assert_eq!(plain.1, "{^\n\n^}cat");
    }

    #[test]
    fn hex_escapes_decode_through_code_page_1252() {
        // 0xE9 is e acute in both cp1252 and Latin-1.
        assert_eq!(one(r"{\*\cxs KAF}caf\'e9").1, "caf\u{e9}");
        // 0x92 is the cp1252 right single quote, not a C1 control.
        assert_eq!(one(r"{\*\cxs AE}it\'92s").1, "it\u{2019}s");
        // 0x80 is the euro sign.
        assert_eq!(one(r"{\*\cxs YUR}\'80").1, "\u{20ac}");
    }

    #[test]
    fn unicode_escapes_decode_and_drop_their_fallback() {
        // The ASCII fallback after the escape is dropped, not doubled.
        assert_eq!(one("{\\*\\cxs AE}it\\u8217 's").1, "it\u{2019}s");
        // Negative parameters are the same code unit written signed.
        assert_eq!(one(r"{\*\cxs AE}\u-16162 ").1, "\u{c0de}");
        // \uc0 means there is no fallback to skip.
        assert_eq!(one("{\\*\\cxs AE}\\uc0\\u8217 x").1, "\u{2019}x");
        // \uc2 skips two fallback characters.
        assert_eq!(one("{\\*\\cxs AE}\\uc2\\u8217 ??x").1, "\u{2019}x");
        // A hex escape counts as one fallback character, not four.
        assert_eq!(one("{\\*\\cxs AE}\\u8217 \\'92s").1, "\u{2019}s");
        // A surrogate pair is one character.
        assert_eq!(one(r"{\*\cxs PHUS}\u-10188 \u-8930 ").1, "\u{1d11e}");
    }

    #[test]
    fn hex_and_unicode_escapes_merge_into_the_surrounding_run() {
        // If the escape split the run, this would become a prefix, not an infix.
        assert_eq!(one(r"{\*\cxs KAF}\cxds caf\'e9\cxds").1, "{^caf\u{e9}^}");
    }

    #[test]
    fn escaped_braces_and_backslashes_survive_as_literal_text() {
        // Braces must arrive escaped or the formatter reads them as a meta.
        assert_eq!(one(r"{\*\cxs PWRAS}\{x\}").1, r"\{x\}");
        assert_eq!(one(r"{\*\cxs PW-S}a\\b").1, r"a\b");
        // The same is true of a brace arriving as a hex escape.
        assert_eq!(one(r"{\*\cxs PWRAS}\'7b").1, r"\{");
    }

    #[test]
    fn deliberate_edge_whitespace_becomes_attach_metas() {
        // Two or more spaces at an edge are intentional and are wrapped so the
        // formatter's own spacing cannot absorb them. One space is not.
        assert_eq!(finalize_translation("  x"), "{^  ^}x");
        assert_eq!(finalize_translation("x  "), "x{^  ^}");
        assert_eq!(finalize_translation(" x "), " x ");
        assert_eq!(finalize_translation("   "), "{^   ^}");
        assert_eq!(finalize_translation("a b"), "a b");
        // A non-breaking space counts as whitespace here, as it does in RTF.
        assert_eq!(one("{\\*\\cxs SP-S}\\'a0\\'a0x").1, "{^\u{a0}\u{a0}^}x");
        assert_eq!(one(r"{\*\cxs SP}\~\~").1, "{^ ^}{^ ^}");
    }

    #[test]
    fn an_entry_with_no_translation_is_kept_as_empty() {
        assert_eq!(one(r"{\*\cxs KAT}"), ("KAT".to_owned(), String::new()));
    }

    // -- malformed input -----------------------------------------------------

    #[test]
    fn a_file_that_is_not_rtf_is_rejected() {
        assert!(matches!(parse("not rtf at all"), Err(RtfError::BadHeader)));
        assert!(matches!(parse(r"{\ansi}"), Err(RtfError::BadHeader)));
        assert!(matches!(parse(""), Err(RtfError::BadHeader)));
    }

    #[test]
    fn an_unterminated_document_is_an_error_not_a_partial_result() {
        let err = parse(r"{\rtf1{\*\cxs KAT}cat").unwrap_err();
        assert!(matches!(err, RtfError::UnexpectedEof { .. }), "{err:?}");
    }

    #[test]
    fn an_unterminated_ignorable_group_is_an_error() {
        let err = parse(r"{\rtf1{\*\cxcomment oops").unwrap_err();
        assert!(matches!(err, RtfError::UnexpectedEof { .. }), "{err:?}");
    }

    #[test]
    fn content_after_the_document_group_is_an_error() {
        let err = parse(r"{\rtf1{\*\cxs KAT}cat}trailing").unwrap_err();
        assert!(matches!(err, RtfError::TrailingContent { .. }), "{err:?}");
    }

    #[test]
    fn a_new_entry_inside_an_open_group_is_an_error() {
        // The group around the first translation was never closed.
        let err = parse(r"{\rtf1{\*\cxs KAT}{cat{\*\cxs TKOG}dog}}").unwrap_err();
        match err {
            RtfError::UnfinishedEntry { open, .. } => assert_eq!(open, 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_truncated_hex_escape_is_an_error() {
        let err = parse(r"{\rtf1{\*\cxs KAT}caf\'e}").unwrap_err();
        assert!(matches!(err, RtfError::Escape { .. }), "{err:?}");
        let err = parse(r"{\rtf1{\*\cxs KAT}caf\'zz}").unwrap_err();
        assert!(matches!(err, RtfError::Escape { .. }), "{err:?}");
    }

    #[test]
    fn a_lone_surrogate_is_an_error_not_a_replacement_character() {
        let err = parse(r"{\rtf1{\*\cxs KAT}\u-10188 x}").unwrap_err();
        assert!(matches!(err, RtfError::Escape { .. }), "{err:?}");
    }

    #[test]
    fn fingerspelling_with_nothing_to_spell_is_an_error() {
        let err = parse(r"{\rtf1{\*\cxs A*}\cxfing}").unwrap_err();
        match err {
            RtfError::ExpectedText { control, .. } => assert_eq!(control, "cxfing"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_trailing_backslash_is_an_error() {
        let err = parse("{\\rtf1 x\\").unwrap_err();
        assert!(matches!(err, RtfError::Escape { .. }), "{err:?}");
    }

    // -- encoding ------------------------------------------------------------

    #[test]
    fn undefined_code_page_1252_bytes_are_reported_not_replaced() {
        for byte in [0x81u8, 0x8D, 0x8F, 0x90, 0x9D] {
            let err = decode_cp1252(&[b'x', byte]).unwrap_err();
            match err {
                RtfError::Encoding { byte: got, .. } => assert_eq!(got, byte),
                other => panic!("{other:?}"),
            }
        }
        // The same byte written as a hex escape is equally an error.
        let err = parse("{\\rtf1{\\*\\cxs KAT}\\'81}").unwrap_err();
        assert!(matches!(err, RtfError::Encoding { .. }), "{err:?}");
    }

    #[test]
    fn code_page_1252_decoding_is_not_latin_1_in_the_c1_range() {
        assert_eq!(decode_cp1252(&[0x93, 0x94]).unwrap(), "\u{201c}\u{201d}");
        assert_eq!(decode_cp1252(&[0xE9]).unwrap(), "\u{e9}");
        assert_eq!(decode_cp1252(b"plain").unwrap(), "plain");
    }

    #[test]
    fn load_reads_a_file_as_code_page_1252() {
        let path = std::env::temp_dir().join("pluvialis_test_rtfcre.rtf");
        // 0xE9 as a raw byte, which is only valid UTF-8 by accident never.
        let mut bytes = br"{\rtf1\ansi{\*\cxs KAF}caf".to_vec();
        bytes.push(0xE9);
        bytes.extend_from_slice(b"}");
        std::fs::write(&path, &bytes).expect("write fixture");

        let got = load(&path).expect("fixture loads");
        assert_eq!(got, vec![("KAF".to_owned(), "caf\u{e9}".to_owned())]);
        std::fs::remove_file(&path).expect("remove fixture");
    }

    #[test]
    fn a_missing_file_reports_its_path() {
        let err = load("F:/definitely/not/here.rtf").unwrap_err();
        assert!(matches!(err, RtfError::Io { .. }), "{err:?}");
    }

    // -- helpers -------------------------------------------------------------

    #[test]
    fn style_control_words_are_told_apart_from_steno_ones() {
        assert!(is_style("s0"));
        assert!(is_style("s12"));
        assert!(!is_style("s"));
        assert!(!is_style("cxs"));
        assert!(!is_style("stylesheet"));
    }

    #[test]
    fn a_realistic_header_and_body_parse_together() {
        let source = concat!(
            "{\\rtf1\\ansi\\ansicpg1252\\deff0{\\*\\cxrev100}",
            "\\cxdict{\\*\\cxsystem Some CAT 4.0}",
            "{\\stylesheet{\\s0 Normal;}}\r\n",
            "{\\*\\cxs KAT}cat\r\n",
            "{\\*\\cxs -G}ing\\cxds \r\n",
            "{\\*\\cxs TP-PL}{\\cxp. }\r\n",
            "{\\*\\cxs KPA}\\cxfc \r\n",
            "}\r\n"
        );
        assert_eq!(
            parse(source).unwrap(),
            vec![
                ("KAT".to_owned(), "cat".to_owned()),
                ("-G".to_owned(), "{ing^}".to_owned()),
                ("TP-PL".to_owned(), "{.}".to_owned()),
                ("KPA".to_owned(), "{-|}".to_owned()),
            ]
        );
    }
}
