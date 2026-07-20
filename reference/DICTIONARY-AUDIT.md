# Dictionary audit

Measured directly from the user's real dictionaries, not assumed. This is the fact base that keeps the M2 formatter scope honest: **implement what is actually used, log loudly what is not.**

Files audited (both live in `C:\Users\Corien\AppData\Local\plover\plover\`):

| File | Entries | Entries with no meta command | Longest entry |
|---|---|---|---|
| `cb_dictionary_full.json` | 93,426 | 92,169 (98.7%) | 15 strokes |
| `corien-dutch.json` | 8,414 | 8,222 (97.7%) | 5 strokes |

**The headline: 98.6% of entries are plain text with no formatting at all.** The formatter is a small surface, not a rewrite of Plover.

`LONGEST_KEY = 15` sets the translator's stroke history window.

---

## Meta commands in use, by category

### Attachment and glue (the bulk of it)

| Form | Uses | Meaning |
|---|---|---|
| `{prefix^}` | 392 | attach to the following word |
| `{^suffix}` | 352 | attach to the preceding word |
| `{^infix^}` | 316 | attach on both sides |
| `{&x}` | 275 | glue: joins to adjacent glue items (fingerspelling, numbers) |

198 distinct glue metas, which is just the alphabet in both cases plus digits and a few symbols. Nothing exotic.

**Orthography matters here.** `{^ing}` applied to "run" must produce "running", not "runing". Plover's rules live in `F:\Steno\plover\plover\orthography.py`. Port those rules; do not hand-roll English suffix logic.

### Capitalization

| Form | Uses | Meaning |
|---|---|---|
| `{>}` | 138 | lowercase the next word |
| `{-|}` | 19 | capitalize the next word |
| `{*-|}` | 2 | retroactively capitalize the last word |
| `{<}` | 1 | uppercase the next word |
| `{~|}` | 1 | capitalize next word but carry the attachment state |

### Punctuation

`{.}` `{,}` `{?}` `{!}` `{;}` `{:}` — roughly 20 uses total. Sentence-ending punctuation attaches to the previous word and capitalizes the next.

### Key combinations

201 uses across **65 distinct combos**. All of them are cursor and editing keys, no application shortcuts:

```
{#Left} {#Right} {#Up} {#Down} {#Home} {#End} {#Delete}
{#Control_L(Left)} {#Control_L(Right)}
{#Shift(Left)} {#Shift(Right)} {#Shift(Up)} {#Shift(Down)}
{#Shift(Home)} {#Shift(End)} {#Shift(Page_Up)} {#Shift(Page_Down)}
{#Control_L(Shift(Left))} {#Control_L(Shift(Right))}
```

Note the nesting: `{#Control_L(Shift(Left))}` means hold Control, hold Shift, press Left. The parser must handle nested parentheses, and the key names are **X11 keysym names** (`Control_L`, `Page_Down`, `BackSpace`), which need mapping to Win32 virtual key codes.

**These need to work in both output paths**: as real keystrokes when another window has focus, and as cursor movements inside the live-type document when Pluvialis has focus. The in-document path is the fiddly one; if a combo has no sensible in-document meaning, log it and drop it rather than guessing.

### Undo and suppression

`{*}` (1), `{*!}` (2), `{*?}` (1) — retroactive operations on the previous translation.

### Modes

`{MODE:CAPS}` (1), `{MODE:RESET}` (1), `{MODE:SET_SPACE:}` (1). Three entries total. Low priority, but cheap.

### Plover engine commands

`{PLOVER:FOCUS}` (4), `{PLOVER:LOOKUP}` (3), `{PLOVER:SUSPEND}` (2), `{PLOVER:RESUME}` (2), `{PLOVER:TOGGLE}` (1), `{PLOVER:ADD_TRANSLATION}` (1), `{PLOVER:CONFIGURE}` (1).

These control the application itself. Map them to Pluvialis equivalents: TOGGLE and SUSPEND/RESUME control output, FOCUS raises the window, LOOKUP and ADD_TRANSLATION open those panes, CONFIGURE opens settings.

### Plugin metas

`{:case:...}` and `{:retro_case:...}` (12 uses, 6 variants: `cap_first_word`, `upper_first_word`, `lower_first_char`, each in normal and retroactive form).

`{:stitch:X}` (52 uses) — fingerspelling that joins with a separator, as in "F-B-I". Comes from the `plover-stitching` plugin.

These are plugin territory in Plover but trivial to implement natively here.

---

## Consequences for the build

1. **`pluvialis check <dict>` must exist** (M2) and must report zero unknown metas across both files before M2 is considered done. That is the acceptance test.
2. **Never silently drop an unrecognized meta.** Log it by exact string. A dropped meta shows up as subtly wrong text hours later, which is the worst possible failure mode for someone writing at speed.
3. **The test corpus for M2 should include at least one entry per row of the tables above**, taken from the real dictionaries so the expected output is verifiable against Plover's actual behavior.
4. The Dutch dictionary uses the same feature set as the English one, so there is no second dialect of formatting to support.

## Reproducing this audit

```
py -X utf8 -c "
import json, re, collections
d = json.load(open('C:/Users/Corien/AppData/Local/plover/plover/cb_dictionary_full.json', encoding='utf-8'))
c = collections.Counter()
for v in d.values():
    for m in re.findall(r'\{[^}]*\}', v): c[m] += 1
for k, n in c.most_common(): print(n, repr(k))
"
```
