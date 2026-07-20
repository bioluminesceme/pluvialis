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

### Manually typed text is never red

**Chosen:** text the user types by hand carries no raw-steno range, and the
document recovers manual edits by diffing the widget's string against its own.

**Rationale:** red means "this steno found no dictionary entry". Typed text has
no outline behind it, so the colour would be meaningless. Existing red ranges
shift, trim and split correctly around manual edits.
