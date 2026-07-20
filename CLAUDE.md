# CLAUDE.md — Pluvialis

Project context for Claude Code. Read this and `PLAN.md` before doing anything.

## What this is

**Pluvialis** is a stenography program in Rust for Windows. It is a standalone program, **not a fork of Plover**. It reuses Plover's protocol knowledge and algorithm design, both read from Plover's source, but shares no code with it.

It exists because Plover's Stenograph plugin cannot recover when the writer is not present at the moment capture starts, which forces the user through a restart-and-reselect ritual every session. See `reference/STENOGRAPH-PROTOCOL.md` section 5 for the three specific bugs.

Named after *Pluvialis*, the golden-plover genus (Latin *pluvia*, rain). Checked for conflicts: Dotterel is a taken Android steno app, Pluvia is heavily used, Pluvialis is clear.

## Status

**Nothing is built yet.** The next session starts at M0 in `PLAN.md`: install rustup, create the Cargo workspace, first commit.

## The user

- Writes steno daily. This is a tool she will rely on, not a toy.
- Hardware: **Luminex CSE** (Stenograph USB) and a **Peregrine** keyboard (Gemini PR over USB serial). The Peregrine is the test machine for most of the build; the Luminex only enters at M8.
- Windows 11 is the 99% case. Linux is a distant maybe. macOS is not a target.
- Her dictionaries live in `C:\Users\Corien\AppData\Local\plover\plover\` and are **shared with her working Plover install, not copied**. Official Plover stays installed as a fallback. Never move, rename, or restructure those files. Editing entries in place is expected; back up before the first write.

## Architecture

Cargo workspace, five crates. The split exists so OS-specific and machine-specific code stays quarantined, which is what keeps a Linux port cheap later.

| Crate | Responsibility | Platform |
|---|---|---|
| `pluvialis-core` | `Stroke`, keymap layer, dictionary load and lookup, translator, formatter, undo history | portable |
| `pluvialis-script` | Lua dictionary host (`mlua`) | portable |
| `pluvialis-machine` | `Machine` trait, Auto scanner, and every protocol | serial/HID portable; Stenograph USB and keyboard hook behind `cfg(windows)` |
| `pluvialis-output` | Keystroke emulation into other apps (Win32 `SendInput`) | Windows now |
| `pluvialis-app` | egui/eframe GUI, documents, autosave, versioning, config | portable |

**Stroke flow:** machine thread reads hardware, sends over a `crossbeam-channel`, translator runs on the UI thread (it is microseconds), produces output actions (backspaces plus text spans, each tagged translated or raw), router dispatches.

**The router is the load-bearing design decision.** Each output batch goes to exactly one destination, decided by window focus at the moment the batch is produced:

- Pluvialis focused: apply to the in-app document at the caret, recording raw-steno spans as byte ranges so the layouter paints them red.
- Anything else focused: emit real keystrokes via `SendInput`.

Never both. That is what makes double-typing structurally impossible rather than a bug to chase.

**Red raw steno persists** because the red ranges live in the document model, not a transient buffer. Steno only ever replaces text via backspaces, so red survives until undone, and undo removes the range with the text.

## Conventions

- **Rust edition 2021**, MSVC toolchain (Visual Studio 2022 Community is already installed at `C:\Program Files\Microsoft Visual Studio\2022\Community`, so only rustup is needed).
- **UTF-8 everywhere.** Every file read and written. If you hit an encoding error, fix the encoding, never paper over it with lossy replacement.
- **No em dashes** anywhere: prose, code comments, docs, commit messages, UI strings. Use a comma, colon, or parentheses.
- **No emoji** in code or output; Windows terminals handle them badly.
- **`py` launcher** for the throwaway Python helper scripts (the jeff-phrasing differential test, dictionary audits). Never `python3`.
- **Comments explain constraints, not narration.** A comment saying what the next line does is noise. A comment saying "cbSize must be 5 here, not the buffer size, per Win32" is worth its space.
- **Fix root causes.** No `unwrap()` to make a borrow checker complaint go away, no swallowing errors to make a test pass.

## Working agreement

The user has been explicit about this, and it matters more than speed:

1. **One milestone at a time.** Commit before starting the next. Do not batch ahead.
2. **Do what was asked, not a better version of it.** If you think there is a better approach, stop and say so, then follow her decision.
3. **After two failed attempts, stop and ask.** Do not try variations of the same idea. Step back and question the assumption instead: am I editing the right file, is my model of the call chain right?
4. **Verify before asserting.** Never claim a file exists, a function behaves a certain way, or a test passes without checking. A slow correct answer beats a fast plausible one.
5. **A test passes only with zero errors and zero warnings.** A warning is a failure.
6. **Commit frequently**, so anything can be reverted cheaply.
7. Keep `PLAN.md` updated as you go, removing completed steps. It is the session-recovery document.

## Verification per milestone

Each milestone in `PLAN.md` names its own check. Two deserve emphasis because they encode the whole point of the project:

- **M4b soak test:** run with no writer attached for ten minutes. Status stays "Searching", one connect attempt per second, CPU near idle, and **handle count flat**. A rising handle count means the Python's `disconnect()` bug was reproduced.
- **M8 hardware matrix:** app-then-writer, writer-then-app, mid-sentence unplug and replug, writer power-cycle. All four must reach connected with no clicks. If any needs user action, the project has not met its goal.

## Reference material

`reference/` holds complete protocol specs transcribed from working implementations, so you should not need to read Python to write the Rust:

- `STENOGRAPH-PROTOCOL.md` — full Luminex USB spec: device GUID, packet format, error codes, read loop, and the three bugs to avoid
- `GEMINI-PR-PROTOCOL.md` — Peregrine, the first machine to implement
- `DICTIONARY-AUDIT.md` — measured meta-command usage in the real dictionaries, which bounds the formatter scope

Plover's source is available for consultation at `F:\Steno\plover\` (v5.4.0) and the Stenograph plugin at `C:\Users\Corien\AppData\Local\plover\plover\plugins\win\Python313\site-packages\`. **Read for reference, never copy code**: Plover is GPL and Pluvialis should be independently written.

Useful Plover files if you need the semantics: `plover/formatting.py`, `plover/translation.py`, `plover/steno.py`, `plover/orthography.py`, `plover/machine/keymap.py`.
