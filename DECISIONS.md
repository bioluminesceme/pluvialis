# Decisions

Judgment calls made while working, with what was rejected and why. Kept so the
user can review and reverse them cheaply rather than discovering them by
archaeology.

Started 2026-07-20, when the user set a goal to finish M5 through M7 unattended.
Anything here is a candidate for reversal; none of it is load bearing in the way
the entries in `thingstonote.md` are.

---

## M5

### The document is a real buffer, with the formatter as a shadow

**Chosen:** keep the formatter formatting the entire stroke history, but treat
its output as a shadow of what steno alone produced. Each stroke diffs the
previous shadow against the new one to get "delete N bytes, insert this text",
and the document applies that edit at the caret.

**Rejected:** making the document a pure function of history (the M3 model). It
is elegant and makes retroactive correction free, but it cannot support a caret
at all: steno can only land at the end, and any manual edit is discarded by the
next stroke.

**Rejected:** having the translator emit edits directly rather than diffing
formatter output. That would mean the formatter could no longer reformat freely,
which is what makes orthography rules work (`{^ing}` on "run" rewrites earlier
text). Diffing costs a string comparison per stroke, which is nothing.

**Consequence to know about:** the shadow and the document can drift if the user
deletes steno-produced text by hand. The next stroke's backspaces are computed
against the shadow, so they may delete text the user did not expect. Backspaces
clamp at the start of the document rather than erroring. Plover has the same
class of issue. If this proves annoying in practice, the fix is to re-anchor the
shadow whenever the user edits manually, which is a small change.

### Auto-scroll follows the end only when the caret is there

**Chosen:** `stick_to_bottom(caret_at_end)`.

**Rejected:** always sticking (the M3 behaviour), which drags the view to the
bottom on every stroke and makes working mid document impossible.

**Rejected:** never sticking, which stops the text following the user while they
write forwards, the common case.

### Backspaces are dropped when focus changes mid correction

**Chosen:** if a batch's destination differs from the previous batch's, its
backspaces are discarded and only the insertion is delivered.

**Rationale:** backspaces refer to text the *previous* batch wrote. If that went
to Notepad and this one goes to our document (or the reverse), deleting would
eat characters this program never wrote, in someone else's document. Losing a
correction is recoverable; silently deleting a stranger's text is not.

**Consequence:** a retroactive correction that straddles an alt-tab leaves the
old word in place and writes the corrected one after it. Rare, visible, and
fixable by hand, which is the right failure direction.

### Key combos only fire when another window has focus

**Chosen:** `{#Control_L(Left)}` is sent only when typing into another
application. When Pluvialis has focus it is logged and skipped.

**Rejected:** synthesising the keystrokes anyway, which would send them to our
own text widget. No dictionary entry means "press Control+Left inside
Pluvialis's document"; they all mean the application being written into.

**Open question for the user:** whether some combos should act on our document
too (Home, End, arrows would all be meaningful). Left unimplemented rather than
guessed at.

### Text is sent as Unicode, combos as virtual keys

**Chosen:** `KEYEVENTF_UNICODE` for text, virtual key codes for combos.

**Rationale:** text means characters, and Unicode events are independent of the
user's keyboard layout, so a Dutch layout and a US layout produce identical
output. Combos mean physical keys, so they have to be virtual key codes.
Navigation keys carry the extended-key flag, which some applications read.

### Manually typed text is never red

**Chosen:** text the user types by hand carries no raw-steno range, and the
document recovers manual edits by diffing the widget's string against its own.

**Rationale:** red means "this steno found no dictionary entry". Typed text has
no outline behind it, so the colour would be meaningless. Existing red ranges
shift, trim and split correctly around manual edits.

---

## M7

### Snapshots are thinned by age, not by count

**Chosen:** keep every snapshot from the last day, one per hour for the last
week, one per day beyond that.

**Rationale:** a count-based cap ("keep the last 50") behaves worst exactly when
it matters most, during a long writing session, where 50 snapshots can cover
twenty minutes. Age banding keeps recent work recoverable minute by minute while
a year of history still costs very little.

**Note:** the bands are aligned to real hour and day boundaries, so two
snapshots a minute apart either side of an hour boundary are two hours as far as
the policy is concerned. This surfaced as two failing tests whose expectations
were written without thinking about boundary alignment; the behaviour is right
and there is now a test pinning it down.

### Recovery is offered, never applied

**Chosen:** after an unclean exit, show a dialog offering the newest snapshot,
with "Start fresh" as an equal option.

**Rejected:** restoring automatically. Silently replacing a blank page the user
meant to start with is its own kind of data loss, and it is the harder one to
undo.

### Crash detection is a marker file

**Chosen:** write `.pluvialis-running` at startup, remove it in `on_exit`. Find
one at startup and the previous run ended badly.

**Verified rather than assumed:** `on_exit` is a real `eframe::App` trait method
and the wgpu integration calls it. A wrong signature would have compiled
silently as an inherent method and simply never run, leaving every clean exit
looking like a crash.

### Documents live in the project folder

**Chosen:** `F:\Steno\Pluvialis\documents\`, with history in
`.pluvialis-history` beside it.

**Rejected:** AppData, which is where Windows would put it but where the user
cannot easily find, back up or edit the files with ordinary tools.

**Worth revisiting:** the path is currently hardcoded. It should be a setting,
and it should not assume drive F.

### Tray icon deferred

The user confirmed it is low priority. The output toggle it was meant to carry
already exists in the status bar, so nothing is lost but the ability to toggle
without focusing the window. Not built.

---

## M6, revised after the user's direction

### Python dictionaries run as they are, rather than being ported

**Chosen:** embed CPython via PyO3 and run Plover's `.py` dictionaries unchanged.

**Decided on measurement, not preference.** `jeff-phrasing.py` answers a lookup
in 2.2us and misses in 1.2us. This project's own Rust JSON lookups measure 0.4us
to 6.5us, and strokes arrive about 200,000us apart at 300wpm. Python is the same
speed as the Rust path and both are irrelevant against the real budget, so the
entire speed argument for porting evaporates.

**Rejected:** porting each dictionary to Rust. It is a week per dictionary, it
can silently drift from the original, and it does nothing for a dictionary
downloaded next year. The user stopped this mid flight and was right.

**Rejected:** converting by enumeration at import. It works only for
dictionaries whose whole output can be enumerated, and it freezes them at
conversion time.

**Two costs, both real.** A Python dictionary is arbitrary code with no sandbox,
the same trust model as Plover. And embedding ties the executable to a Python
installation (CPython 3.12 at `C:\Python312`), which reverses the original
"single exe, no Python" goal. The user chose this knowingly.

### Python dictionaries load disabled

**Chosen:** any `.py` in the dictionary folder is discovered and appears in the
list with a checkbox, switched **off**.

**Rationale:** the user asked to import any Python dictionary and enable or
disable it, and separately said she is not sure she wants jeff-phrasing yet. Off
by default satisfies both: they are visible and one click away, and nothing
about her existing outlines changes until she asks.

### JSON dictionaries stay JSON

**Chosen:** no conversion of the user's JSON dictionaries to anything.

**Rationale:** they are already native (101,407 entries in 52ms, lookups under
7us), they are shared in place with the working Plover install that is her
fallback, and she wants to keep editing her main dictionary in VS Code. A flat
`"KAT": "cat"` map is the most editable form available. Converting would break
Plover compatibility and gain nothing.

### The Lua host was cut

**Decided by the user, 2026-07-20:** "If python works and json works, we do not
need lua."

It was built earlier the same day, before measurement established that Python
dictionaries run at full speed. Once they did, Lua's only remaining advantage
was that its scripts are sandboxed and Python's are not, which is not worth a
whole dictionary format nobody would write in.

`crates/pluvialis-script` and its `mlua` dependency are gone. The crate was moved
to the Recycle Bin rather than deleted outright, and it is in git history at
d603c60 if it is ever wanted back. Removing it also drops the vendored Lua C
build from the toolchain.

The sandbox lesson from it is worth keeping even though the code is not:
selecting standard libraries was not enough, because the base library still
carried `dofile` and `loadfile`, and both read files from disk. If a sandboxed
scripting layer ever returns, test the escape rather than assuming the library
selection covers it.

### The jeff-phrasing Rust port was removed

**Decided by the user, 2026-07-20:** "Jeff phrasing rust port can be removed (or
archive until we've confirmed in the final app that the python dictionary works
well)."

Archived rather than deleted, which satisfies both halves: `phrasing.rs` and
`phrasing_dictionary.rs` are in the Recycle Bin and in git history at 67276c6~.

The confirmation she asked for was run first, not assumed. The differential test
against the real `jeff-phrasing.py` through embedded CPython passes over all
218,071 enumerated outlines, 173,785 of them answered. That is the same corpus
the Rust port was validated against, so Python demonstrably covers what the port
covered. What is still unconfirmed is the GUI path (ticking the dictionary on and
writing with it), which needs her hands.

The fixture moved to `crates/pluvialis-python/tests/` because it now proves the
Python reader rather than the port. It stays at 6.8 MB: it is the only evidence
that the whole calling convention (tuple packing, return extraction, `KeyError`
meaning "no entry") is right, and regenerating it needs her Python file.

### Python dictionaries are screened before they are executed

**Found while removing the port, and it was writing to her files.**

`PythonDictionary::load` ran the module body and only then checked for a `lookup`
function, and the app discovers dictionaries by scanning her Plover folder. Three
`.py` files live there and one is a dictionary. `backupcoriendict.py` copies
`cb_dictionary.json` to `F:\Steno` from module level, so every GUI start ran that
copy. Nothing was corrupted, and the script does only what its name says, but the
app had no business running it.

The root cause is the discovery model, not the check. Plover runs the `.py` files
its config names; scanning a folder is friendlier and means meeting files that
were never meant to be dictionaries. The check now reads the source for a
`lookup` definition before executing anything.

**This is not a sandbox and must not be described as one.** Anything that passes
the screen is still arbitrary code with full access to the machine, exactly as in
Plover. It only stops the app running files nobody asked it to run.

### RTF/CRE is read, not converted

Written against Plover's `rtfcre_parse.py` as a specification, independently
implemented. Returns `Vec<(outline, translation)>` with translations already in
the meta syntax `format.rs` understands, so an imported RTF behaves like any
other dictionary.

Two deliberate differences from Plover, both recorded in the module:

- A literal brace is emitted escaped (`\{`), because `format.rs` would otherwise
  read it as the start of a meta command. Plover has the same hazard and does not
  guard against it.
- Plover's stylesheet branch appends whatever text was left over from a previous
  loop iteration, because it never assigns its own local. That is a bug whose
  output happens to be discarded downstream. Not replicated.

**Untested against a real commercial dictionary.** There is no `.rtf` anywhere on
`F:\Steno` to try, so every input is hand-constructed from the shapes in Plover's
reader and writer. The first real file may still surprise it. Ten of the tests
are written separately from the module's own, on the principle that a suite
written alongside the code shares its blind spots; one of those ten was wrong
about `\cxds \cxds` and the parser was right, checked against `rtfcre_parse.py`.

**Two integration decisions are still open**, both deferred until the dictionary
copying question is settled:

- `format.rs` does not understand `=undo` or `=macro_name` translations, which
  `\cxdstroke` and `\cxplovermacro` produce. `parse_atoms` sees no braces, so
  they would be typed out as literal text.
- Outlines come back as raw strings. Whoever builds a `Dictionary` from them
  must decide whether unparseable keys go to `bad_keys`, as `Dictionary::load`
  does for JSON.

### Pluvialis owns its dictionaries

**Decided by the user, 2026-07-20:** "Better to copy any dictionary that gets
imported to the program folder so we never accidentally edit a source that is
also used by something else." Confirmed when asked which reading she meant: "any
dictionaries we open in Pluvialis get saved into a special folder. User can edit
them in VSCode from there if needed, and Pluvialis has full access too. We own
them."

This reverses the earlier decision to share her files in place with Plover, and
it was asked about rather than assumed, because she had previously been explicit
that her main dictionary should stay a solo file she edits in VS Code. It still
is. It just lives in `F:\Steno\Pluvialis\dictionaries\` now.

**The cost was stated before she chose, and she chose it anyway:** the copies
drift. Editing her Plover copy no longer reaches Pluvialis, and vice versa.

**Seeding happens once,** when the folder does not exist. Not on every start: a
dictionary she deliberately removed would otherwise come back. Once created, the
folder is hers, and adding a dictionary means putting the file in it.

A side effect worth naming: the app no longer scans her Plover folder at all, so
the class of bug where it executed a `.py` that was never meant to be a
dictionary is gone at the root rather than guarded against. The screen stays as
defence in depth, and it is what decides which `.py` files are worth seeding.

**Priority order is alphabetical by file name.** Predictable beats clever when
the user is the one dropping files in the folder, and it happens to preserve the
seeded pair's order, which is load bearing. A test pins that coincidence so it
fails loudly rather than silently if a future dictionary needs to outrank
`cb_dictionary_full`.

### Her plover.cfg says something different from what this project assumed

**Found on 2026-07-20 by reading it, and the note it corrects had been repeated
several times.** `DICTIONARIES` was documented as "mirroring her plover.cfg"
with `cb_dictionary_full.json` and `corien-dutch.json`. Her actual config is:

    cb_dictionary_full.json   enabled
    cb_dictionary.json        disabled
    mouse.json                disabled
    jeff-phrasing.py          ENABLED

So `corien-dutch.json` is not in her Plover config at all, and `jeff-phrasing.py`
is switched on there.

The second half matters more than the first. Pluvialis loads Python dictionaries
disabled, and the comment justifying that cites her saying she is not sure she
wants jeff-phrasing. What she actually said was "Don't make the Jeff's phrasing
native please, I'm not sure I'll use it yet", which was about the Rust port. Her
config suggests she writes with the dictionary daily. **Not flipped, because the
default carries an explicit instruction not to change it without asking, and
because being wrong here changes what her writing produces.** Raised with her.

### RTF/CRE wiring is parked

**Decided by the user, 2026-07-20:** "Add RTF to plan for later, I don't use it
and if I don't release this on github implementation is not needed."

The reader stays: it is written, tested, and costs nothing dormant. Only the
wiring is deferred, and it is on the post-1.0 list where a public release would
need it, since RTF/CRE is how commercial dictionaries ship.

### Pluvialis does contain Plover material, and the docs said otherwise

**Raised by the user, 2026-07-20:** "We did copy a lot of Plover's logic right?
So just copy their license to and say this is a Rust port of Plover??"

She was right to push, and the claim she was pushing against was mine. `CLAUDE.md`
had said Pluvialis "shares no code with" Plover since M0, and it was repeated
through several milestones without being checked. Checking it found two pieces of
Plover material:

- `assets/american_english_words.txt`, **byte identical** to Plover's asset
  (verified with `cmp`), 338,882 lines, embedded in the executable.
- `orthography_rules.rs`, 38 orthography rules and their aliases transcribed from
  `plover/system/english_stenotype.py`, with the file's own header saying so.

So Pluvialis is a derivative work in part, and GPL is an obligation rather than
only a preference.

**The licence needed no change.** Plover is GPL-2.0-**or-later**, so its material
may be used under GPL-3, and GPL-3.0-or-later was already declared. The
conclusion was right for a reason that had not been established.

**"A Rust port of Plover" was rejected**, though the user offered it. It claims
more derivation than exists: the translator, formatter, machine layer, document
model and storage are independently designed, and several behave differently on
purpose, including the machine scanner that this project exists to fix. It would
also attribute this program's bugs to Plover's authors. `ATTRIBUTION.md` states
what came from where, and is now the authoritative record; the source files carry
the same notice so provenance travels with them.

The general lesson is the one already in this project's working agreement, and
this is the third time it has paid out: a confident claim in a document is not
evidence. `cmp` is.
