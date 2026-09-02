//! A flat, searchable view of every dictionary entry, for the Dictionary
//! table.
//!
//! The dictionaries themselves are hash maps built for one job: answer an
//! outline in nanoseconds while the user is writing. Browsing needs the
//! opposite shape, an ordered list that can be scanned, so this builds one
//! beside them rather than changing them.
//!
//! Two rules keep it fast at the real size, 101,419 entries across two files:
//!
//! - Rendering an outline allocates, so every outline is rendered once when the
//!   index is built and never again.
//! - No comparison sort at query time. The display orders are permutations
//!   computed once, so a search is a single pass that pushes matching ids in
//!   the order they will be shown.

use std::path::{Path, PathBuf};

use pluvialis_core::{DictionaryStack, Stroke};

/// One entry, flattened for display.
pub struct Entry {
    /// Canonical rendering, done once at build time.
    pub outline: String,
    pub word: String,
    /// Index into the index's dictionary names and paths.
    pub dictionary: u16,
}

/// Which column the table is ordered by.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sort {
    /// Best match first, then shortest outline. The order for a search, where
    /// what was asked for matters more than the alphabet.
    #[default]
    Relevance,
    Outline,
    Word,
    Dictionary,
}

impl Sort {
    /// The permutation a sort reads. Relevance walks outline order and lets the
    /// ranking do the rest.
    fn column(self) -> usize {
        match self {
            Sort::Relevance | Sort::Outline => 0,
            Sort::Word => 1,
            Sort::Dictionary => 2,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub text: String,
    pub sort: Sort,
    /// Reversed by clicking an already-selected column header. Ignored by
    /// `Relevance`, where "least relevant first" is not a thing anyone wants.
    pub descending: bool,
    /// Show only one dictionary, when set.
    pub dictionary: Option<u16>,
}

/// How well an entry answers a query. Lower is better.
///
/// Case is folded for ASCII only. Outlines are ASCII by definition, and folding
/// the Dutch dictionary's accented letters would need a Unicode table for a
/// gain nobody has asked for. An accented search still matches exactly.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Exact,
    Prefix,
    Contains,
}

const RANKS: usize = 3;

#[derive(Default)]
pub struct EntryIndex {
    entries: Vec<Entry>,
    names: Vec<String>,
    paths: Vec<PathBuf>,
    /// Display orders, by [`Sort::column`]. Built when a sort is first used,
    /// because most sessions never click a column header and sorting 101,419
    /// strings three times at startup would be paid for by everyone.
    orders: [Option<Vec<u32>>; 3],
    cached: Option<Query>,
    rows: Vec<u32>,
    /// Counted so a test can prove a repeated query does no work.
    scans: usize,
}

impl EntryIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read every entry out of the stack. Call after anything that changes what
    /// the dictionaries contain.
    pub fn rebuild(&mut self, stack: &DictionaryStack) {
        self.entries.clear();
        self.names.clear();
        self.paths.clear();
        self.orders = [None, None, None];
        self.cached = None;
        self.rows.clear();

        for (index, dictionary) in stack.dictionaries().iter().enumerate() {
            // More dictionaries than this and the row layout is the problem,
            // not the index.
            let Ok(index) = u16::try_from(index) else {
                log::warn!("more than 65,535 dictionaries, ignoring the rest");
                break;
            };
            self.names.push(short_name(&dictionary.path));
            self.paths.push(dictionary.path.clone());

            for (strokes, word) in dictionary.entries() {
                self.entries.push(Entry {
                    outline: Stroke::render_outline(strokes),
                    word: word.to_owned(),
                    dictionary: index,
                });
            }
        }
    }

    pub fn total_entries(&self) -> usize {
        self.entries.len()
    }

    pub fn total_matches(&self) -> usize {
        self.rows.len()
    }

    /// The ids to draw, in display order.
    pub fn rows(&self) -> &[u32] {
        &self.rows
    }

    pub fn entry(&self, id: u32) -> Option<&Entry> {
        self.entries.get(id as usize)
    }

    pub fn dictionaries(&self) -> impl Iterator<Item = (u16, &str)> {
        self.names
            .iter()
            .enumerate()
            .map(|(index, name)| (index as u16, name.as_str()))
    }

    pub fn name(&self, dictionary: u16) -> &str {
        self.names
            .get(dictionary as usize)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn path(&self, dictionary: u16) -> Option<&Path> {
        self.paths.get(dictionary as usize).map(PathBuf::as_path)
    }

    /// Recompute the visible rows, unless this exact query already produced
    /// them.
    pub fn refresh(&mut self, query: &Query) {
        if self.cached.as_ref() == Some(query) {
            return;
        }
        self.build_order(query.sort);
        self.scans += 1;

        let needle = query.text.trim().to_ascii_lowercase();
        let order = self.orders[query.sort.column()]
            .as_deref()
            .unwrap_or_default();

        // Ranked buckets, so the whole thing stays one pass with no sort. An
        // unranked search (no text) has one bucket and copies the permutation.
        let mut buckets: [Vec<u32>; RANKS] = Default::default();
        for &id in order {
            let entry = &self.entries[id as usize];
            if let Some(only) = query.dictionary
                && entry.dictionary != only
            {
                continue;
            }
            match rank(entry, &needle) {
                None => continue,
                Some(rank) => buckets[rank as usize].push(id),
            }
        }

        self.rows.clear();
        for bucket in &mut buckets {
            self.rows.append(bucket);
        }
        // Relevance has no meaningful reverse, so it is left alone.
        if query.descending && query.sort != Sort::Relevance {
            self.rows.reverse();
        }
        self.cached = Some(query.clone());
    }

    fn build_order(&mut self, sort: Sort) {
        let column = sort.column();
        if self.orders[column].is_some() {
            return;
        }
        let mut ids: Vec<u32> = (0..self.entries.len() as u32).collect();
        match column {
            // Shortest outline first within the same text, since the brief is
            // what a writer wants to find.
            0 => ids.sort_by(|a, b| {
                let (a, b) = (&self.entries[*a as usize], &self.entries[*b as usize]);
                a.outline
                    .len()
                    .cmp(&b.outline.len())
                    .then_with(|| a.outline.cmp(&b.outline))
            }),
            1 => ids.sort_by(|a, b| {
                let (a, b) = (&self.entries[*a as usize], &self.entries[*b as usize]);
                a.word
                    .to_ascii_lowercase()
                    .cmp(&b.word.to_ascii_lowercase())
                    .then_with(|| a.outline.cmp(&b.outline))
            }),
            _ => ids.sort_by(|a, b| {
                let (a, b) = (&self.entries[*a as usize], &self.entries[*b as usize]);
                a.dictionary
                    .cmp(&b.dictionary)
                    .then_with(|| a.outline.cmp(&b.outline))
            }),
        }
        self.orders[column] = Some(ids);
    }
}

/// How well one entry answers the query, or `None` for no match.
///
/// An empty query matches everything at one rank, which is what makes the
/// unsearched table the whole dictionary in display order.
fn rank(entry: &Entry, needle: &str) -> Option<Rank> {
    if needle.is_empty() {
        return Some(Rank::Exact);
    }
    if entry.outline.eq_ignore_ascii_case(needle) || entry.word.eq_ignore_ascii_case(needle) {
        return Some(Rank::Exact);
    }
    if starts_with_fold(&entry.outline, needle) || starts_with_fold(&entry.word, needle) {
        return Some(Rank::Prefix);
    }
    if contains_fold(&entry.outline, needle) || contains_fold(&entry.word, needle) {
        return Some(Rank::Contains);
    }
    None
}

/// `needle` is already ASCII lowercased by the caller, once, rather than per
/// entry.
fn starts_with_fold(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    haystack.len() >= needle.len() && eq_fold(&haystack[..needle.len()], needle)
}

fn contains_fold(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| eq_fold(w, needle))
}

fn eq_fold(a: &[u8], lowered: &[u8]) -> bool {
    a.iter()
        .zip(lowered)
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

fn short_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pluvialis_core::Dictionary;

    fn temp_dict(name: &str, json: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pluvialis-index-{name}-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, json).unwrap();
        path
    }

    fn index(paths: &[&Path]) -> EntryIndex {
        let mut stack = DictionaryStack::new();
        for path in paths {
            stack.push(Dictionary::load(path).unwrap());
        }
        let mut index = EntryIndex::new();
        index.rebuild(&stack);
        index
    }

    fn words(index: &EntryIndex) -> Vec<&str> {
        index
            .rows()
            .iter()
            .filter_map(|id| index.entry(*id))
            .map(|e| e.word.as_str())
            .collect()
    }

    fn outlines(index: &EntryIndex) -> Vec<&str> {
        index
            .rows()
            .iter()
            .filter_map(|id| index.entry(*id))
            .map(|e| e.outline.as_str())
            .collect()
    }

    fn query(text: &str) -> Query {
        Query {
            text: text.to_owned(),
            ..Query::default()
        }
    }

    const SAMPLE: &str = r#"{
"KAT": "cat",
"SKALD": "scald",
"KAERT": "cart",
"TKOG": "dog"
}
"#;

    #[test]
    fn an_empty_query_lists_everything_shortest_outline_first() {
        let path = temp_dict("all", SAMPLE);
        let mut index = index(&[&path]);
        index.refresh(&query(""));

        assert_eq!(index.total_entries(), 4);
        assert_eq!(index.total_matches(), 4);
        assert_eq!(outlines(&index), ["KAT", "TKOG", "KAERT", "SKALD"]);
    }

    #[test]
    fn a_substring_matches_inside_a_word() {
        // The whole point of the change: "ca" has to find "scald", which an
        // exact or prefix match never would.
        let path = temp_dict("substring", SAMPLE);
        let mut index = index(&[&path]);
        index.refresh(&query("ca"));

        let found = words(&index);
        assert!(found.contains(&"cat"), "{found:?}");
        assert!(found.contains(&"cart"), "{found:?}");
        assert!(found.contains(&"scald"), "{found:?}");
        assert!(!found.contains(&"dog"), "{found:?}");
    }

    #[test]
    fn a_substring_matches_inside_an_outline() {
        let path = temp_dict("outlinesub", SAMPLE);
        let mut index = index(&[&path]);
        index.refresh(&query("AL"));
        assert_eq!(outlines(&index), ["SKALD"]);
    }

    #[test]
    fn exact_ranks_before_prefix_before_contains() {
        let path = temp_dict(
            "ranked",
            r#"{
"KAT": "cat",
"KAERT": "catalogue",
"SKALD": "scat"
}
"#,
        );
        let mut index = index(&[&path]);
        index.refresh(&query("cat"));

        assert_eq!(words(&index), ["cat", "catalogue", "scat"]);
    }

    #[test]
    fn capitalisation_does_not_hide_a_match() {
        let path = temp_dict("case", "{\n\"THE\": \"The\"\n}\n");
        let mut index = index(&[&path]);
        index.refresh(&query("the"));
        assert_eq!(words(&index), ["The"]);
    }

    #[test]
    fn a_miss_reports_none_of_the_total_honestly() {
        let path = temp_dict("miss", SAMPLE);
        let mut index = index(&[&path]);
        index.refresh(&query("zzzz"));

        assert_eq!(index.total_matches(), 0);
        assert_eq!(index.total_entries(), 4, "the total is still the truth");
    }

    #[test]
    fn sorting_by_word_orders_alphabetically_and_reverses() {
        let path = temp_dict("byword", SAMPLE);
        let mut index = index(&[&path]);

        index.refresh(&Query {
            sort: Sort::Word,
            ..Query::default()
        });
        assert_eq!(words(&index), ["cart", "cat", "dog", "scald"]);

        index.refresh(&Query {
            sort: Sort::Word,
            descending: true,
            ..Query::default()
        });
        assert_eq!(words(&index), ["scald", "dog", "cat", "cart"]);
    }

    #[test]
    fn filtering_to_one_dictionary_hides_the_others() {
        let english = temp_dict("filten", "{\n\"KAT\": \"cat\"\n}\n");
        let dutch = temp_dict("filtnl", "{\n\"KAT\": \"kat\"\n}\n");
        let mut index = index(&[&english, &dutch]);

        index.refresh(&Query {
            dictionary: Some(1),
            ..Query::default()
        });
        assert_eq!(words(&index), ["kat"]);
    }

    #[test]
    fn repeating_a_query_does_no_work() {
        let path = temp_dict("cached", SAMPLE);
        let mut index = index(&[&path]);

        index.refresh(&query("ca"));
        let after_first = index.scans;
        index.refresh(&query("ca"));
        index.refresh(&query("ca"));

        assert_eq!(index.scans, after_first, "the same query is answered once");
    }

    #[test]
    fn rebuilding_answers_the_next_query_afresh() {
        let path = temp_dict("rebuild", SAMPLE);
        let mut stack = DictionaryStack::new();
        stack.push(Dictionary::load(&path).unwrap());

        let mut index = EntryIndex::new();
        index.rebuild(&stack);
        index.refresh(&query("ca"));
        let before = index.scans;

        index.rebuild(&stack);
        index.refresh(&query("ca"));

        assert_eq!(index.scans, before + 1, "the cache did not survive");
    }

    /// Not a test, a measurement. Run with
    /// `cargo test -p pluvialis-app --release cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement, not a pass or fail"]
    fn cost_at_the_real_size() {
        use std::fmt::Write as _;
        use std::time::Instant;

        let mut json = String::from("{\n");
        for i in 0..101_419u32 {
            let _ = writeln!(json, "\"KAT/{i}\": \"word number {i}\",");
        }
        json.push_str("\"KAT\": \"cat\"\n}\n");
        let path = temp_dict("cost", &json);

        let mut stack = DictionaryStack::new();
        let started = Instant::now();
        stack.push(Dictionary::load(&path).unwrap());
        println!("load          {:?}", started.elapsed());

        let mut index = EntryIndex::new();
        let started = Instant::now();
        index.rebuild(&stack);
        println!("rebuild       {:?}", started.elapsed());

        let started = Instant::now();
        index.refresh(&query(""));
        println!(
            "empty query   {:?}  ({} rows)",
            started.elapsed(),
            index.total_matches()
        );

        let started = Instant::now();
        index.refresh(&query("number 7"));
        println!(
            "substring     {:?}  ({} rows)",
            started.elapsed(),
            index.total_matches()
        );

        let started = Instant::now();
        index.refresh(&Query {
            sort: Sort::Word,
            ..Query::default()
        });
        println!("sort by word  {:?}", started.elapsed());

        let bytes: usize = index
            .entries
            .iter()
            .map(|e| e.outline.len() + e.word.len() + 56)
            .sum();
        println!("approx bytes  {} MB", bytes / 1_048_576);
        let _ = std::fs::remove_file(&path);
    }
}
