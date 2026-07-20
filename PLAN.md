# Pluvialis: a Rust steno program for the Luminex CSE

> **Start here.** Nothing is built yet. The next session begins at **M0** below.
>
> Read `CLAUDE.md` first (architecture, conventions, working agreement), then the specs in `reference/` for whichever milestone you are on. Those specs are complete transcriptions, so you should not need to read Plover's Python at all.
>
> Keep this file updated as you go: strike completed milestones, record any decision that deviates from the plan and why. This is the session-recovery document.
>
> **Everything below the milestones is settled research**, not open questions. It was gathered by reading source and measuring the user's real dictionaries. If something here contradicts what you observe, trust your observation and update this file, but say so rather than quietly diverging.

## Context

Plover with a Luminex CSE is unreliable on this setup: every session needs fiddling (restart the writer, press a key, reopen Plover settings, re-select "Stenograph USB", hope it connects). The root cause is in Plover's Stenograph plugin, which gives up permanently if the writer is not present the moment capture starts, and never retries on its own.

Rather than patch Plover, we build a separate program, **Pluvialis**, in Rust. It reuses Plover's *ideas* and wire protocol (both GPL-compatible knowledge we have read from source) but shares no Python code. Goals:

- Connects to the Stenograph USB writer with a scan loop that retries forever and never needs a dialog, so it just works whenever the machine is on. (This started as "hardcode the machine"; it became an Auto scanner once we decided to support more protocols, which solves the same pain better.)
- Live-type window as the main screen: big text area, steno tape strip on the right, untranslated strokes shown in red as raw steno.
- Multiple JSON dictionaries with priority order and enable/disable, plus dictionary edit and lookup.
- Lua for programmatic ("scripty") dictionaries, so things like jeff-phrasing have a future.
- Markdown documents with autosave, versioned history, and crash recovery.
- Single .exe, no venv, no Python.
- Eventually every protocol Plover supports, so this can be released publicly. Build order follows what you actually use.

Name check done: Dotterel is taken (an Android steno app), Pluvia is heavily used. **Pluvialis** (the golden-plover genus, from Latin *pluvia*, rain) has no software conflicts.

**Hardware answer to your question:** you do not need to plug the Luminex into this PC yet. Everything up to M7 is built and tested without it, using your **Peregrine** as the test machine from M4a onward. The Luminex and its driver install are only needed at M8. Plugging it in earlier does no harm, it just will not be used.

---

## Verified facts this plan rests on

Read from source, not assumed:

- **The USB protocol is small and fully readable.** `stenograph/transport_windows.py` (251 lines) opens the writer through SetupAPI using device interface GUID `{c5682e20-8059-604a-b761-77c4de9d5dbf}`, then plain `CreateFile`/`ReadFile`/`WriteFile`. `packet.py` defines the whole wire format: header `<2sIH6I` = `"SG"`, u32 sequence, u16 packet type, u32 data length, five u32 params; packet types OPEN_FILE `0x11`, READ_FILE `0x13`, ERROR `0x6`; error codes NO_REALTIME_FILE `8`, FINISHED_READING_CLOSED_FILE `9`. Strokes are 8-byte chords (4 steno bytes, 4 timestamp), each steno byte having its low 6 bits map onto the `STENO_KEY_CHART` rows in `stroke.py`. This is roughly 150 lines of Rust with the `windows` crate.
- **Two real bugs in that Python transport explain the fiddling**, and we simply do not reproduce them: `disconnect()` sets the handle to INVALID *before* closing it (so it leaks the real handle and then closes garbage), and `connect()` returns `False` instead of erroring when the writer is absent. Combined with `start_capture()` in `plover_stenograph/base.py` calling `self._error()` and never starting its thread when the writer is missing at startup, that is exactly the "reopen settings and re-select the machine" ritual.
- **Your dictionaries are almost entirely plain text.** `cb_dictionary_full.json`: 93,426 entries, 92,169 with no meta command at all, longest entry 15 strokes. `corien-dutch.json`: 8,414 entries, 8,222 plain, longest 5 strokes. The meta commands actually in use are a short list: prefix/suffix/infix attachment (930 uses), glue (275), key combos (201), capitalize `{-|}` and `{>}` (approximately 350), punctuation, a handful of `{PLOVER:...}` and `{MODE:...}`, and a few plugin metas (`stitch`, `case`, `retro_case`). We implement the used set and log anything unrecognized rather than guessing at Plover's full surface.
- **Toolchain is nearly ready.** MSVC is already installed (Visual Studio 2022 Community, `C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35207`), so Rust needs only `rustup`. Cargo is not currently installed.
- **egui supports the red-raw-steno requirement directly.** `TextEdit::layouter` takes a closure returning a `LayoutJob`, which carries per-range colors, so raw strokes render red without a custom text widget.
- Reference implementations exist but are not reusable as-is: [plojo](https://github.com/richyliu/plojo) is Rust and GPL-3.0 but has no GUI and no Stenograph support. We look at it for algorithm sanity checks only, and keep Pluvialis independently written.

---

## Architecture

Cargo workspace at `F:\Steno\Pluvialis`, five crates so the machine-specific and OS-specific parts stay isolated (this is what keeps a future Linux build cheap):

| Crate | Responsibility | Platform |
|---|---|---|
| `pluvialis-core` | `Stroke`, keymap layer, dictionary loading and lookup, translator, formatter/meta commands, undo history | portable |
| `pluvialis-script` | Lua dictionary host (`mlua`) | portable |
| `pluvialis-machine` | `Machine` trait plus every protocol: Stenograph USB, Gemini PR, TX Bolt, ProCAT, Passport, Plover HID, Stentura, keyboard | serial and HID are portable; Stenograph USB and the keyboard hook are Windows-specific behind `cfg` |
| `pluvialis-output` | Keystroke emulation into other apps (Win32 `SendInput`) | Windows now |
| `pluvialis-app` | egui/eframe GUI, documents, autosave, versioning, config | portable |

**Stroke flow:** machine thread reads USB → `crossbeam-channel` → translator (on the UI thread, it is microseconds) → produces a list of output actions (backspaces + text, each span tagged translated or raw) → router.

**The router is the one design decision that matters.** Each output batch goes to exactly one destination depending on focus, checked at the moment the batch is produced:

- Pluvialis window focused → apply to the in-app document (insert text at the caret, delete backspaces), with raw-steno spans recorded as byte ranges so the layouter paints them red.
- Any other window focused → hand to `pluvialis-output`, which emits real keystrokes via `SendInput`, exactly like Plover.

This is your stated rule ("we type where the cursor is") and it also sidesteps a nasty class of bug: we never do both, so there is no double-typing.

**Raw strokes stay red permanently.** The red ranges live in the document model, not in a transient buffer. Since steno only ever *replaces* text via backspaces, red text stays red until you undo it away, and if you undo it the range is removed with it.

**Translation algorithm** (same shape as Plover): keep the last N strokes (N = longest key in the loaded dictionaries, currently 15); on each new stroke, search longest-match-first across the stroke history; if a longer match is found retroactively, undo the previously emitted output for those strokes and emit the new translation. Dictionaries are searched in priority order, first hit wins, disabled ones skipped. `*` alone is undo.

---

## Machines

The end state is full protocol coverage matching Plover, so this is releasable on GitHub as a real alternative. The build order is driven by what you personally use: **Stenograph USB and Gemini PR ship in the main sequence; everything else is a post-1.0 list.**

The `Machine` trait and the keymap layer (machine-specific key names mapped to system actions, Plover's `keymap.py` concept) are built first regardless, because retrofitting them later is expensive and they cost almost nothing up front. Once they exist, each extra protocol is an isolated file that touches nothing else, which is exactly why deferring them is safe.

**In the main sequence (M4):**

| Protocol | Plover source | What speaks it | Effort |
|---|---|---|---|
| Gemini PR | 56 lines | **your Peregrine**, most DIY boards | trivial, and it gives us a real test machine on day one |
| Stenograph USB | 251 lines (plugin) | **your Luminex CSE** | the main event |

**Post-1.0 list, in the order I would add them** (each is self-contained, none blocks anything):

| Protocol | Plover source | What speaks it | Effort |
|---|---|---|---|
| TX Bolt | 92 lines | many writers, common fallback mode | a few hours |
| ProCAT | 51 lines | ProCAT writers | a few hours |
| Passport | 64 lines | Passport writers | a few hours |
| Plover HID | 333 lines | QMK/ZMK boards, 64 levers, plug-and-play | about a day, needs `hidapi` |
| Keyboard | 171 lines plus OS layer | your QWERTY keyboard | fiddly: global low-level hook with key suppression |
| Stentura | 685 lines | older Stentura writers | the heavy one: sequenced packets, checksums |

**Machine selection improves on Plover deliberately**, since machine selection is the source of your current pain. Default mode is **Auto**: on start and whenever the current machine drops, Pluvialis scans for any machine it knows how to talk to, in a priority order you can set (Stenograph USB first by default), and connects to whatever it finds. It keeps scanning forever rather than giving up. You can also pin a specific machine in settings if Auto ever picks wrong. The practical effect: turn the Luminex on and it connects; plug the Peregrine in instead and that connects; no dialog, no restart.

Plover's own HID implementation already works this way (a device scan loop with hot-plug handling), so it doubles as a reference for the scanner.

---

## Dictionary story

- **JSON stays first-class.** Plover JSON is loaded natively, no conversion, no format change. Your existing files in `C:\Users\Corien\AppData\Local\plover\plover\` are read in place (shared, not copied), so official Plover keeps working as a fallback.
- **Lua is the scripty layer.** A `.lua` dictionary in the list exposes `lookup(strokes) -> string | nil` and optionally `reverse_lookup(text)`. It is called only when the JSON dictionaries above it miss, so the cost is near zero in normal writing.
- **Conversion tools** ship as subcommands of the same exe, since you asked for them built in:
  - `pluvialis convert rtf <in.rtf> <out.json>` (RTF/CRE, the format most commercial dictionaries come in)
  - `pluvialis convert json-to-lua <in.json> <out.lua>` (for when you want to make a static dictionary programmatic)
  - `pluvialis check <dict>` (report entries using meta commands we do not implement, so nothing fails silently)
- **jeff-phrasing** gets a native Rust port (Milestone 6). It is 811 lines but mostly lookup tables (`STARTERS`, `MIDDLES`, `STRUCTURES`, `ENDERS` and friends) driven by one regex-based `determine_parts`. The port is validated by differential testing: a throwaway Python script enumerates every stroke the original responds to, and a Rust test asserts identical output for all of them. That is the only honest way to know a port is correct, and it is cheap here because `LONGEST_KEY = 1` bounds the space.

## Documents

Markdown, as you asked. Autosave to `F:\Steno\Pluvialis\documents\` on an interval you set (default 60 seconds) and on focus loss. Versioning without requiring git to be installed: each autosave that differs from the last writes a timestamped snapshot under `.pluvialis-history/<docname>/`, with a retention policy (keep every version for 24 hours, then hourly for a week, then daily). A history pane lets you view and restore any snapshot. Crash recovery is the same mechanism: on start, if a session ended without a clean save, offer the newest snapshot.

---

## Milestones

Each one ends with something you can run and check, and gets its own git commit.

**On hardware:** you do not need the Luminex for any milestone before M8, and you do not need to plug it into this PC at all until then. From M4a onward the **Peregrine** is the test machine, which is better than a fake dev input because it exercises the real stroke path. Plugging the Luminex in earlier does no harm, it just will not be used until its driver is installed at M8.

### ~~M0. Toolchain and skeleton~~ DONE (commit `a071294`)
rustc 1.97.1 MSVC, clippy and rustfmt installed. Workspace with all five crates builds; `pluvialis-app` opens a window titled Pluvialis; `cargo clippy --workspace --all-targets -- -D warnings` exits 0.

One deviation from the plan as written, recorded in `thingstonote.md`:
- **Edition 2024**, not 2021, with `rust-version = "1.97"` (the floor gates dependency resolution; too low a value silently held eframe at 0.33).

Everything is on latest published versions: rustc 1.97.1, eframe and egui 0.35.0. eframe uses default features, including the `wgpu` renderer. A `wgpu-hal` compile failure seen once during M0 did not reproduce and is noted as transient.

Remaining dependencies get added in the milestone that needs them, rather than up front: `serde`/`serde_json` (M1), `crossbeam-channel` and `serialport` (M4a), `windows` (M4b), `mlua` (M6).

### ~~M1. Core: strokes, dictionaries, translation~~ DONE
`crates/pluvialis-core/src/{stroke,dictionary,translator}.rs`, 26 tests, clippy clean.

Measured against the real dictionaries via `pluvialis-app.exe lookup KAT WEL/KO*PL TPHRPBLG`:
- 101,407 entries across both dictionaries **loaded in 52 ms**, longest key 15
- lookups at **400 ns to 4 µs**, so translation latency is nowhere near being a concern
- retroactive correction confirmed: `WEL` gives "well", then `KO*PL` withdraws it for "welcome"

Two findings, both in `thingstonote.md`:
- **433 keys in `corien-dutch.json` are not valid steno** (doubled `U`, `*` after `-E`/`-U`). Plover's own `plover-stroke` rejects them identically, so this is broken data, not a parser bug. They are skipped and reported.
- Our parser was checked against `plover-stroke` 1.1.0 and agreed on every case tried.

`jeff-phrasing.py` is not loadable by the CLI (it is Python); the native port is M6.

### ~~M2. Formatter: spaces, capitalization, meta commands~~ DONE
`crates/pluvialis-core/src/{format,orthography,orthography_rules}.rs`, 58 tests, clippy clean.

`pluvialis check` formats all **101,407 entries in 52 ms** and exits 0. Every meta command that works in her Plover works here.

- **Orthography** ported from Plover: 38 rules generated by `tools/gen_orthography.py` straight from Plover's source rather than transcribed, plus the 338,882 word frequency list embedded via `include_str!` so the exe stays self contained. Gives "running", "writing", "artistically", "happier".
- **12 meta uses remain unimplemented and that is correct.** `{*}`, `{*!}`, `{*?}` are absent from Plover 5.4's own dispatch table; `{:emoji}` and `{:stitch_last_word:*}` need plugins that are not installed. All are dead in her Plover too. `check` treats this set as a baseline and only fails on something new, which makes it a regression test.
- **`fancy-regex`, not `regex`**: one orthography rule uses a negative look-behind, which `regex` does not support.
- Found and fixed a real bug via `check`: `{:}` was being swallowed as an empty macro instead of falling through to colon punctuation. Plover's pattern `:([^:]+):?(.*)` requires a non-empty name.

Formatting reformats the whole history per call rather than incrementally, trading microseconds for removing a class of stale state bugs. `Formatted::raw_ranges` carries the byte ranges of untranslated strokes, ready for M3 to paint red.

### M2 original scope (for reference)
Implement exactly the meta set your dictionaries use, verified by the audit above: `{^suffix}`, `{prefix^}`, `{^infix^}`, `{&glue}`, `{-|}`, `{>}`, punctuation (`{.}` `{,}` `{?}` `{!}` `{;}` `{:}`), `{}`, `{#key combos}`, `{*!}`, `{*-|}`, `{*}`, `{~|}`, and the `{MODE:...}`/`{PLOVER:...}` entries present. Anything unrecognized is logged by name, never silently dropped. Orthography rules for suffixes (the "-ing" doubling rules and friends).
**Verify:** a test corpus of stroke sequences with expected output, including every meta command found in your two dictionaries. `pluvialis check` reports zero unknown metas across both files.

### M3. Live-type window
Main window: `TextEdit` with a custom layouter, tape strip on the right (recent strokes, raw steno plus translation), connection status bar. Raw untranslated strokes render red. Document model with red-range tracking and correct backspace handling. Temporary dev input: a text box where you type a raw stroke and press Enter, so the pipeline is exercisable before any machine exists (removed once M4a lands).
**Verify:** type `KAT` in the dev box, "cat" appears; type nonsense like `TPHRPBLG`, it appears red as raw steno; type `*`, the red text disappears; correct spacing and capitalization after punctuation.

### M4a. Machine trait, keymap layer, and Gemini PR
The `Machine` trait (connect, stroke stream, status), the keymap layer, the Auto scanner with its forever-retry loop, and Gemini PR as the first implementation (serial port, 9600 baud, six-byte packets with the MSB-as-first-byte rule).
**Verify with real hardware, today:** plug in the Peregrine, launch Pluvialis, and write. Strokes appear in the live view, unknown chords come out red, `*` undoes. This is the first end-to-end proof and it needs nothing from Stenograph.

### M4b. Stenograph USB, with a connect loop that does not give up
Port the transport to Rust with the `windows` crate: SetupAPI enumeration by the writer GUID, `CreateFile` on the device path, packet pack/unpack, `send_receive`, stroke decoding. Then the state machine the Python version lacks: retry connect once per second forever, open `REALTIME.000`, poll with read requests, and on any I/O error reset state and drop back to retrying. Unplug, replug, power-cycle, and app-started-before-machine all converge to connected without user action.
**Verify without the machine:** run it, confirm the status shows "Searching for writer", the log shows one attempt per second, CPU stays near idle, and handle count is flat after ten minutes (this specifically proves we did not reproduce Plover's handle leak). Real-machine verification is M8.

### M5. Output routing, dictionary tools, tray
`SendInput` keystroke emulation for when another window has focus. Focus-based routing. Tray icon with an output on/off toggle. Dictionary pane (list with priority order, drag to reorder, enable/disable checkboxes), lookup window, and an entry editor that writes back to the JSON files.
**Verify:** with Notepad focused, strokes type into Notepad; click back into Pluvialis, strokes go into the live view at the caret; toggle output off, nothing types anywhere; edit an entry, confirm the JSON file on disk changed and the new translation takes effect immediately.

### M6. Lua dictionaries, conversion tools, jeff-phrasing port
`mlua` host with the `lookup`/`reverse_lookup` contract and a sandbox (no filesystem or network from dictionary scripts). The `convert` and `check` subcommands. Native Rust jeff-phrasing with the differential test against the Python original.
**Verify:** a toy Lua dictionary resolves strokes the JSON files miss; the jeff-phrasing differential test passes for every enumerated stroke; phrasing strokes work in the live view.

### M7. Documents, autosave, versioning, crash recovery
Markdown save/open, autosave interval, snapshot history with retention, history browser and restore, crash recovery prompt.
**Verify:** write, wait for autosave, kill the process from Task Manager, restart, confirm the text is recovered; restore an older snapshot from the history pane.

### M8. Real hardware
Install the Stenograph driver from `F:\Steno\StenoMachines\USB_Writer_Drivers\`. Close official Plover (both would open the writer). Then the matrix that matters, which is the whole point of this project:
1. App running, writer off → turn writer on and start writing → connects on its own, no clicks.
2. Writer on first, then app start → connects immediately.
3. Unplug USB mid-sentence → status changes, replug → resumes.
4. Power-cycle the writer → resumes.
5. Auto mode picks the Luminex when both it and the Peregrine are attached, per the priority order.
6. Full session: real strokes through `cb_dictionary_full.json`, red raw steno for unknown chords, typing into another program, autosave.
Then a release build and a desktop shortcut. **This is 1.0 for your daily use.**

---

## Post-1.0 list

Not needed for your setup, but wanted for a public GitHub release. Each is self-contained behind the `Machine` trait and can be done in any order, whenever you feel like it.

1. **TX Bolt** (a few hours) — the most widely supported fallback protocol, so it buys the most compatibility per hour.
2. **ProCAT** and **Passport** (a few hours each) — same shape as TX Bolt, share the serial helper.
3. **Plover HID** (about a day) — QMK/ZMK boards. Also the upgrade path if you ever reflash the Peregrine.
4. **Keyboard machine** (fiddly) — global low-level Windows hook with key suppression, for writing steno on QWERTY.
5. **Stentura** (the heavy one) — sequenced packets with checksums, for old Stentura writers.
6. **Linux build** — the core, serial and HID crates are already portable; this is a Stenograph libusb transport plus an X11/Wayland output layer.

Release chores whenever you decide to publish: LICENSE (GPL-3.0 is the natural fit given the lineage), README with the supported-machines table, and a GitHub Actions job producing a signed Windows .exe.

---

## Critical files

**In this repo, written before M0 and ready to use:**
- `CLAUDE.md` — architecture, conventions, working agreement
- `thingstonote.md` — the traps, organised by milestone. Read your milestone's section before writing its code.
- `reference/STENOGRAPH-PROTOCOL.md` — complete Luminex USB spec for M4b, including the three bugs to avoid
- `reference/GEMINI-PR-PROTOCOL.md` — complete Peregrine spec for M4a
- `reference/DICTIONARY-AUDIT.md` — measured meta-command usage, which bounds the M2 formatter

**To read from (reference only, never modified):**
- `C:\Users\Corien\AppData\Local\plover\plover\plugins\win\Python313\site-packages\stenograph\` — `transport_windows.py`, `packet.py`, `stroke.py` are the protocol spec for M4b
- `C:\Users\Corien\AppData\Local\plover\plover\plugins\win\Python313\site-packages\plover_stenograph\base.py` — the read loop and its failure modes
- `F:\Steno\plover\plover\machine\gemini_pr.py` — protocol spec for M4a; `keymap.py` and `base.py` for the keymap layer
- `F:\Steno\plover\plover\machine\` — `tx_bolt.py`, `procat.py`, `passport.py`, `plover_hid.py`, `stentura.py`, `keyboard.py` for the post-1.0 list
- `F:\Steno\plover\plover\formatting.py`, `plover\translation.py`, `plover\steno.py` — reference semantics for M1 and M2
- `F:\Steno\jeff-phrasing.py` — source for the M6 port

**To create:** everything under `F:\Steno\Pluvialis\`.

**Shared, read and written but never moved:** the dictionary JSON files in `C:\Users\Corien\AppData\Local\plover\plover\`.

---

## Risks, stated plainly

- **Writer handshake timing is the one thing I cannot verify from source.** The read loop logic is faithfully ported from code that works in your frozen Plover, but whether the Luminex responds identically to a Rust caller can only be confirmed at M8. If it misbehaves there, the fallback is to compare against a packet log from working Plover. Note this risk is now contained: the Peregrine proves the whole rest of the pipeline at M4a, so if M8 has trouble it is isolated to one file.
- **This is a real reimplementation.** Plover's formatter has years of edge cases in it. The dictionary audit says your actual usage is narrow, which makes this tractable, but expect a tail of small formatting differences to fix as you write with it. `pluvialis check` and the unknown-meta logging exist so those surface loudly instead of quietly.
- **egui's text editing is simpler than a word processor.** Selection, undo of manual typing, and IME are more basic than Qt or a native control. For an append-mostly live-type view this is fine; if it chafes later, the document model is separate from the widget, so swapping the view is contained.
- **Estimate: M0 through M5 is the bulk of the value** and is where a working daily driver appears. M6 and M7 are additive, and the post-1.0 protocol list is optional forever unless you publish.

## Working agreement

One milestone at a time, committed before the next starts. If something fails twice, I stop and bring it to you rather than trying variations. UTF-8 everywhere, no em dashes, `py` launcher for the throwaway Python helper scripts.
