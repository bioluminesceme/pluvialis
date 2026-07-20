# CLAUDE.md — Pluvialis

Project context for Claude Code. Read this and `PLAN.md` before doing anything.

## What this is

**Pluvialis** is a stenography program in Rust for Windows. It is a standalone program, **not a fork of Plover**. It reuses Plover's protocol knowledge and algorithm design, both read from Plover's source, but shares no code with it.

It exists because Plover's Stenograph plugin cannot recover when the writer is not present at the moment capture starts, which forces the user through a restart-and-reselect ritual every session. See `reference/STENOGRAPH-PROTOCOL.md` section 5 for the three specific bugs.

Named after *Pluvialis*, the golden-plover genus (Latin *pluvia*, rain). Checked for conflicts: Dotterel is a taken Android steno app, Pluvia is heavily used, Pluvialis is clear.

## Status

**M0 through M4b are done and verified on real hardware. M5 is next.**

Both writers work. The Luminex, the user's primary machine, connects at 0.0s, is preferred over the Peregrine when both are attached, translates correctly against her real dictionaries, and renders unknown chords red in the live view. Neither machine needs a dialog or a machine selection, which is the goal this project exists for. 117 tests, clippy clean at `-D warnings`.

One deferred, non-blocking check is listed under M4b in `PLAN.md`: the writer-off soak test for the searching path.

**Do not trust this project's own reference documents over the machine.** Three M4b facts recorded in `reference/STENOGRAPH-PROTOCOL.md` and `thingstonote.md` turned out to be wrong (the device interface GUID, `cbSize`, and an unnoticed fourth Plover bug), each found by checking against the registry, `ctypes.sizeof`, or the source rather than trusting the note. All three are now corrected, and all three would have presented as "the writer is not plugged in".

**The Luminex is the user's primary machine**, so M4b comes before M5 even though M5 (typing at the caret) is the more visible feature. Its driver is already installed and bound, which removes the biggest unknown from M4b and M8.

The translation core is complete and measured against the real dictionaries: 101,407 entries load in 52 ms, lookups run 400 ns to 4 us, and `pluvialis check` formats every entry in 52 ms and exits 0. Latency is a settled non issue; do not re-litigate it.

Working commands (`target\release\pluvialis-app.exe <cmd>`):
- `lookup <OUTLINE>...` — answer from the real dictionaries, with timings
- `check [DICT...]` — format every entry, report unimplemented meta commands. Exits 0 on the known baseline, non zero on anything new, so it is a regression test.
- `clean [--write] [DICT...]` — remove entries whose keys are not valid steno. Dry run unless `--write`.
- `machine [SECONDS]` — run the Auto scanner and print every status change and stroke. The machine layer is otherwise only visible through the GUI, which makes a connection problem hard to see and impossible to paste anywhere. `RUST_LOG=pluvialis_machine=trace` adds per-attempt detail.
- no arguments — opens the GUI

Run tests with `cargo test --workspace`, lint with `cargo clippy --workspace --all-targets -- -D warnings`. Both must be clean before a milestone is done.

## The user

- Writes steno daily. This is a tool she will rely on, not a toy.
- Hardware: **Luminex CSE** (Stenograph USB) and a **Peregrine** keyboard (Gemini PR over USB serial). **The Luminex is her primary machine**; the Peregrine is only "much faster to set up". Both work as of M4b. Keep both attached when testing Auto mode, since that is what exercises the priority order.
- Windows 11 is the 99% case. Linux is a distant maybe. macOS is not a target.
- **Pluvialis owns its dictionaries, in `F:\Steno\Pluvialis\dictionaries\`.** Decided by the user on 2026-07-20: "any dictionaries we open in Pluvialis get saved into a special folder. User can edit them in VSCode from there if needed, and Pluvialis has full access too. We own them." The folder is seeded **once**, from `C:\Users\Corien\AppData\Local\plover\plover\`, when it does not yet exist; after that nothing re-copies into it, or a dictionary she removed would come back on the next start. Adding a dictionary means putting the file in the folder.
- **This supersedes the earlier "shared in place, never copied" rule**, which was correct until she changed it. The cost is real and she accepted it knowingly: the two copies drift, so editing her Plover copy no longer changes Pluvialis and vice versa.
- Her Plover folder is now read **only** to seed on first run, and never written to.
- **Her actual `plover.cfg` list**, read on 2026-07-20 rather than assumed: `cb_dictionary_full.json` (enabled), `cb_dictionary.json` (disabled), `mouse.json` (disabled), `jeff-phrasing.py` (**enabled**). Earlier notes in this project claimed the pair `cb_dictionary_full.json` plus `corien-dutch.json` "mirrors her plover.cfg". That was wrong: `corien-dutch.json` is not in her Plover config at all.

## Architecture

Cargo workspace, five crates. The split exists so OS-specific and machine-specific code stays quarantined, which is what keeps a Linux port cheap later.

| Crate | Responsibility | Platform |
|---|---|---|
| `pluvialis-core` | `Stroke`, keymap layer, dictionary load and lookup, translator, formatter, undo history | portable |
| `pluvialis-machine` | `Machine` trait, Auto scanner, and every protocol | serial/HID portable; Stenograph USB and keyboard hook behind `cfg(windows)` |
| `pluvialis-output` | Keystroke emulation into other apps (Win32 `SendInput`) | Windows now |
| `pluvialis-app` | egui/eframe GUI, documents, autosave, versioning, config | portable |

**Stroke flow:** machine thread reads hardware, sends over a `crossbeam-channel`, translator runs on the UI thread (it is microseconds), produces output actions (backspaces plus text spans, each tagged translated or raw), router dispatches.

**The router is the load-bearing design decision.** Each output batch goes to exactly one destination, decided by window focus at the moment the batch is produced:

- Pluvialis focused: apply to the in-app document at the caret, recording raw-steno spans as byte ranges so the layouter paints them red.
- Anything else focused: emit real keystrokes via `SendInput`.

Never both. That is what makes double-typing structurally impossible rather than a bug to chase.

**Red raw steno persists** because the red ranges live in the document model, not a transient buffer. Steno only ever replaces text via backspaces, so red survives until undone, and undo removes the range with the text.

## Licence

**GPL-3.0-or-later.** Confirmed by the user on 2026-07-20 ("Yes to GPL3", then
"or just use whatever license plover uses, I'm fine with anything really"). Both
answers land in the same place: Plover is `GPL-2.0-or-later`, and the "or later"
makes GPL-3 compatible with it.

The choice is free rather than inherited. Pluvialis contains no Plover code, so
nothing obliges it to be GPL at all; it is GPL because the user chose it. Keep it
that way: **read Plover for reference, never copy from it.** The moment code is
copied, the licence stops being a choice and the "independently written" claim in
this file becomes false.

`LICENSE` holds the canonical FSF text, fetched verbatim from gnu.org. Do not
edit it. All five crates inherit `license.workspace = true`.

## Conventions

- **Rust edition 2024**, MSVC toolchain, `rust-version = "1.92"`. Installed: rustc 1.97.1, clippy, rustfmt. Visual Studio 2022 Community provides the linker.
- **`$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH";`** is needed to prefix cargo commands in fresh shells until the profile is reloaded.
- **eframe uses default features, including the `wgpu` renderer.** Verified building and running on this machine.
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
8. **Correct wrong reference material the moment you find it.** Standing instruction from the user, 2026-07-20. If something in `reference/`, `thingstonote.md`, `PLAN.md` or this file turns out to be factually wrong, fix the document as part of the same task rather than routing around it in code. A confidently wrong note is worse than no note: it sends the next session down a path that fails. Say plainly that the old entry was wrong and what the evidence was, and prefer ground truth (the registry, the running system, a measured value) over restating whatever source was wrong in the first place. Three M4b facts were wrong this way, and each presented as "the writer is not plugged in".

## Verification per milestone

Each milestone in `PLAN.md` names its own check. Two deserve emphasis because they encode the whole point of the project:

- **M4b soak test:** run with no writer attached for ten minutes. Status stays "Searching", one connect attempt per second, CPU near idle, and **handle count flat**. A rising handle count means the Python's `disconnect()` bug was reproduced.
- **M8 hardware matrix:** app-then-writer, writer-then-app, mid-sentence unplug and replug, writer power-cycle. All four must reach connected with no clicks. If any needs user action, the project has not met its goal.

## Writing to the user's dictionaries

Writes now land on Pluvialis's own copies in `dictionaries\`, which is the whole reason the library exists: nothing else reads the file being edited. Her Plover folder is never written to.

That lowers the stakes but does not change the rules, because these are still the only copy of her working vocabulary. Anything that modifies a dictionary follows the pattern established by `pluvialis-core::clean`:

1. **Dry run by default.** Writing is opt in (`--write`), never a side effect of running a report.
2. **Verify before writing.** Reparse the result and check entry count and every retained value. On any mismatch, write nothing and return an error.
3. **Keep the original and whatever was removed**, as timestamped siblings, so the edit is always reversible.
4. **Preserve formatting.** Both dictionaries store one entry per line; edit lines rather than reserializing, or a two entry change produces a 93,000 line diff.

## Read `thingstonote.md`

`thingstonote.md` collects the traps: places where the correct code looks like a bug, where Plover's code looks fine and is broken, and where an instinct to tidy something will break it. Several entries exist specifically because the urge to "fix" them is strong. Read the section for whichever milestone you are on, before writing that milestone's code rather than after debugging it.

## Reference material

`reference/` holds complete protocol specs transcribed from working implementations, so you should not need to read Python to write the Rust:

- `STENOGRAPH-PROTOCOL.md` — full Luminex USB spec: device GUID, packet format, error codes, read loop, and the three bugs to avoid
- `GEMINI-PR-PROTOCOL.md` — Peregrine, the first machine to implement
- `DICTIONARY-AUDIT.md` — measured meta-command usage in the real dictionaries, which bounds the formatter scope

Plover's source is available for consultation at `F:\Steno\plover\` (v5.4.0) and the Stenograph plugin at `C:\Users\Corien\AppData\Local\plover\plover\plugins\win\Python313\site-packages\`. **Read for reference, never copy code**: Plover is GPL and Pluvialis should be independently written.

Useful Plover files if you need the semantics: `plover/formatting.py`, `plover/translation.py`, `plover/steno.py`, `plover/orthography.py`, `plover/machine/keymap.py`.
