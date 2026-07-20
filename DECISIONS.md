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

### Open question: does the Lua host still have a purpose?

It was built before we established that Python runs natively at full speed. Its
one remaining advantage is that Lua scripts are sandboxed and Python
dictionaries are not. If the user would never write a Lua dictionary herself, it
is a feature with no user and should be cut rather than maintained. Raised with
her, not yet answered.
