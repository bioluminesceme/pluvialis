# Things to note

Traps a fresh session will walk into. Each one is something that looks wrong and is right, looks right and is wrong, or is invisible until it costs an hour.

Organised by where you will hit it. Read the section for the milestone you are on.

---

## The kind of trap this file is about

Several of these are cases where **the correct code looks like a bug**. If you find yourself "cleaning up" something in this list, stop: it was deliberate and it is documented here precisely because the instinct to fix it is strong.

The opposite case also appears: code in Plover's Python that looks fine and is actually broken. We do not copy it.

---

## Windows API (M4b, Stenograph USB)

**The device interface GUID is not the one written in Plover's source.** `transport_windows.py` spells `{c5682e20-8059-604a-b761-77c4de9d5dbf}`, but it builds the value from `uuid.UUID(...).bytes` (big endian) and hands the raw bytes to Win32, whose `GUID` is little endian in its first three fields. The real value, confirmed against this machine's registry, is **`{202e68c5-5980-4a60-b761-77c4de9d5dbf}`**. Using the string as written enumerates nothing and looks exactly like the writer being switched off, so it is a trap that costs an afternoon. Also do not reach for the INF's `ClassGuid` (`...6980...`): that is the setup class, one hex digit away and a different concept.

**`cbSize` is the struct size, not the buffer size.** In `SetupDiGetDeviceInterfaceDetailA`, `SP_DEVICE_INTERFACE_DETAIL_DATA_A.cbSize` is **8** on x64 (a `DWORD` plus one `CHAR`, padded to 4 byte alignment) and 5 on x86 where the struct is packed, however large the buffer you allocated. Use `size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_A>()` and it is right on both. Setting it to the allocated size yields `ERROR_INVALID_USER_BUFFER`, which hints at nothing. (An earlier version of this file said 5 on x64. That was wrong, and it was wrong in the confident direction: `ctypes.sizeof` on the Python's own struct measures 8.)

**Allocate the detail buffer as `Vec<u32>`, not `Vec<u8>`.** `cbSize` is written through that pointer, and a misaligned `u32` write is undefined behaviour. The device path then starts at `offset_of!(..., DevicePath)`, which is 4, not at the struct's padded size of 8.

**`CREATE_ALWAYS | CREATE_NEW` is not a flags bug, but do not copy it.** The Python passes `2 | 1 == 3` to `CreateFileA`, and 3 happens to be `OPEN_EXISTING`. It works entirely by numeric coincidence. Write `OPEN_EXISTING` in Rust. If you see the Python and "fix" it to a real flag combination, you will change the value and break it.

**The first `SetupDiGetDeviceInterfaceDetailA` call is expected to fail.** You call it with a null buffer to learn the required size, and it fails with `ERROR_INSUFFICIENT_BUFFER (0x7A)`. That is the success path. Only treat any *other* error as real.

**`ERROR_NO_MORE_ITEMS (0x103)` from `SetupDiEnumDeviceInterfaces` means "no writer plugged in".** This is the normal idle state, not an error. It happens once per second forever while the machine is off. Log it at trace level at most, never warn, or the log becomes unreadable and the user thinks something is broken.

**Close the handle before invalidating it.** Plover's `disconnect()` does it backwards:
```python
self._usb_device = INVALID_HANDLE_VALUE   # real handle now lost
if not CloseHandle(self._usb_device):     # closes -1, fails, raises
```
This leaks a handle on every disconnect. In a forever-retry loop that is a slow resource exhaustion. Take the handle out first, then close it. The M4b soak test (ten minutes disconnected, handle count flat in Task Manager) exists solely to prove we did not repeat this.

---

## Stenograph protocol (M4b)

**`data_length` includes the padding.** Payloads are zero-padded to a multiple of 8, and the length is computed *after* padding. Do not compute it from the unpadded data.

**Error code 8, `NO_REALTIME_FILE`, is completely normal.** It means the user has not started writing yet. Same for code 9, `FINISHED_READING_CLOSED_FILE`. Both are routine, both mean "reset state and keep polling", neither should surface to the user as a problem. Treating 8 as a failure produces software that looks broken whenever it is merely idle.

**Strokes read before `realtime` is true are discarded on purpose.** When you first open `REALTIME.000` there is a backlog already in the file. You read it, advance the offset, and throw it away, until a zero-length response tells you that you have caught up to live. This looks like a bug ("we are receiving strokes and ignoring them") and is not. Emitting them would dump the user's previous session's text into the document on every connect.

**The steno byte bit order runs high to low.** Each of the 4 steno bytes has its top two bits always set, and the low 6 bits are keys where **bit 5 (value 32) is the first key in the row and bit 0 (value 1) is the last**. It is `1 << (5 - j)`, not `1 << j`. Get this backwards and every stroke mirrors into a different, plausible-looking, wrong stroke.

**Response must echo the request's sequence number and packet type, *except* when it is an error.** An ERROR packet carries type `0x06` and so never echoes the request type. Check `is_error` first and dispatch on the code; only then apply the sequence and type checks, and only to non-error responses.

Plover gets this backwards and it is probably the bug that bites the user: it checks sequence and type *before* looking at the code, so every error becomes an uncaught `ProtocolViolationException` that ends the reader thread. Its `except NoRealtimeFileException` handler is unreachable dead code. Since code 8 just means "not writing yet", this fires in the most ordinary situation there is. See `reference/STENOGRAPH-PROTOCOL.md` section 5, bug 4.

**`^` is a machine key, not steno, and not the attachment `^`.** It is the first entry in the writer's key chart and has no steno meaning; the Stentura keymap binds it to `no-op`, so `Keymap::stentura()` leaves it unbound. This is unrelated to the `^` in dictionary values (`{^ing}`, `{^zaam}`), of which the user's dictionaries hold 959. They can never collide: outlines are dictionary keys, attachment lives in values, and no valid steno outline can contain `^` (measured: zero keys in either dictionary do).

---

## Gemini PR (M4a)

**The duplicate keys are not redundancy to optimise away.** `S1-`/`S2-` are the two halves of the S key, `*1` through `*4` the four star keys, `#1` through `#C` the number bar segments. All 42 chart entries are distinct machine keys. Collapsing them to `S-`, `*`, `#` is the **keymap layer's** job, which is exactly why the keymap layer must exist before this machine is written. Hardcoding the collapse in the Gemini decoder feels simpler and will make every later machine wrong.

**The bit test is `b & (0x80 >> j)` for `j` in 1..8, and the chart index is `i * 7 + j - 1`.** Seven bits per byte, not eight, because the MSB is the framing marker. The off-by-one here is very easy and produces strokes that are wrong but look reasonable.

**Assert DTR (and RTS) after opening the port.** A USB CDC device commonly treats DTR as "a host is actually listening" and sends nothing without it. The port opens, reports healthy, the app shows connected, and not one byte arrives. This cost real debugging time on 2026-07-20 because the symptom is identical to the user simply not writing. `serialport` does not do this for you: call `write_data_terminal_ready(true)` after `open()`.

**The Luminex offers a serial port too, and it is a trap.** With both machines attached the Peregrine is `VID_FEED&PID_6060` on one COM port, and the Luminex is `VID_112B&PID_000D` presenting "Stenograph Writer Serial Port" on another. That second port is **silent** (verified: zero bytes while writing on the Luminex). An Auto scanner that opens it reports a healthy Gemini PR connection that never delivers a stroke, and the open handle blocks the Stenograph implementation from the same device. `gemini::OTHER_PROTOCOL_VIDS` excludes VID `0x112B` for exactly this reason. **The exclusion must be checked before the remembered-port preference**, or an unplug-replug that reassigns COM numbers revives the bug.

**Do not hold candidate serial ports open.** Auto-detection means opening ports to see whether steno comes out. A port you hold is a port other software cannot use. Open, sniff for a valid packet, and release promptly if it is not a steno device. Remember the VID/PID that worked so later scans go straight to it.

---

## Dictionaries and formatting (M1, M2)

**The 433 invalid keys were removed on 2026-07-20** with `pluvialis clean --write`. The originals are at `corien-dutch.backup-1784541960.json` and the removed entries at `corien-dutch.removed-1784541960.json`, both in the Plover config directory. Verified afterwards: 7981 + 433 = 8414, kept entries byte identical, nothing lost. The history below explains why they were invalid.

**`corien-dutch.json` had 433 keys that are not valid steno, and Plover rejects them too.** They look like `STKPWEU**URT` and `WEU*UF`: a doubled `U`, or a `*` placed after `-E`/`-U` when the canonical order is `A- O- * -E -U`. Verified against Plover's own `plover-stroke` 1.1.0, which raises `ValueError: invalid steno` on the same keys. These entries have never worked in Plover either, and that dictionary is not in her enabled list. **Do not "fix" the parser to accept them.** We skip them, count them, and print the first few.

**To settle any parser disagreement, test against Plover's real implementation** rather than reasoning about it:
```
py -m pip install --target "$env:TEMP\pstroke" plover-stroke==1.1.0
py -X utf8 -c "
import sys, os; sys.path.insert(0, os.environ['TEMP']+'/pstroke')
from plover_stroke import BaseStroke
class S(BaseStroke): pass
S.setup(('#','S-','T-','K-','P-','W-','H-','R-','A-','O-','*','-E','-U','-F','-R','-P','-B','-L','-G','-T','-S','-D','-Z'), ('A-','O-','*','-E','-U'), '#')
print(S.from_steno('KAT').keys())
"
```
Our parser matched Plover on every case tried, including rendering `TPHRPBLG` as `TPHR-PBLG`.

**Canonical rendering is not what was typed.** `TPHRPBLG` has no center key, so a hyphen is required to mark the right bank and it renders as `TPHR-PBLG`. Raw untranslated steno in the live view will therefore sometimes not look like the keys pressed. This is correct and matches Plover; do not add a "preserve original spelling" path.

**`LONGEST_KEY` is 15, not the usual 10.** Measured from `cb_dictionary_full.json`. The translator's stroke history window must accommodate it or long entries silently never match.

**Never silently drop an unrecognised meta command.** Log it by exact string. A dropped meta does not crash, it produces subtly wrong text that the user discovers hours later in a document. For someone writing at speed, that is the worst available failure mode. `pluvialis check` reporting zero unknowns across both dictionaries is the M2 acceptance test.

**Do not hand-roll English suffix orthography.** `{^ing}` on "run" must give "running", not "runing". Port the rules from `F:\Steno\plover\plover\orthography.py`. This looks like a small string operation and is a large pile of accumulated special cases.

**Key combo names are X11 keysyms, not Win32 names.** `Control_L`, `Page_Down`, `BackSpace`. They need mapping to virtual key codes. They also nest: `{#Control_L(Shift(Left))}` means hold Control, hold Shift, press Left, so the parser needs real nesting, not a split on parentheses.

**98.6% of entries are plain text with no meta at all.** If the formatter is turning into a large subsystem, re-read `reference/DICTIONARY-AUDIT.md`. The real surface is small and measured.

---

## Toolchain and egui (M0 onward, hit during M0)

**A `wgpu-hal` build failure during M0 did not reproduce, so treat it as transient.** The first build attempt died in `wgpu-hal` with a wall of Direct3D12 `Param`/`CanInto` trait errors against `windows` 0.62. Re-testing the identical configuration later (same `Cargo.lock`, same wgpu-hal 29.0.4, same windows 0.62.2, release profile, default eframe features) built clean and ran fine. The most likely cause is a partially resolved dependency graph while the manifest was being edited from eframe 0.33.3 to 0.35, but that is a guess and it did not reproduce. **We use eframe's default features, including the `wgpu` renderer.** If those D3D12 errors reappear, try `cargo clean` and a fresh resolve before concluding anything is genuinely incompatible.

**egui 0.35 replaced `App::update` with `App::ui`.** The trait is now:
```rust
fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame);   // required
fn logic(&mut self, ctx: &egui::Context, frame: &mut Frame) { }  // optional, no painting
```
`CentralPanel::show` correspondingly takes `&mut Ui`, not `&Context`. **Essentially every egui example online still shows the old `update(&mut self, ctx: &Context, ...)` form**, so copying a snippet will not compile and the error (`method 'update' is not a member of trait`) does not point at the reason. Put non-drawing per-frame work in `logic`, drawing in `ui`.

**egui 0.35 unified the panels into one type.** `SidePanel` and `TopBottomPanel` no longer exist; it is `egui::Panel::left/right/top/bottom(id)`, and the width setter is `default_size`, not `default_width`/`default_height`. `CentralPanel` is unchanged. The compile error is a plain "could not find `TopBottomPanel` in `egui`", which does not hint that a replacement exists.

**`LayoutSection::byte_range` is `Range<ByteIndex>`, not `Range<usize>`.** `ByteIndex` is a newtype re-exported as `egui::text::ByteIndex`; build ranges with `ByteIndex(start)..ByteIndex(end)` and read them back through `.start.0`. It also does not index a `str`, so slicing needs `&text[r.start.0..r.end.0]`.

**`TextEdit::frame` takes a `Frame`, not a `bool`.**

**`Color32::PLACEHOLDER` in a `TextFormat` means "use the widget's text colour".** `painter.galley(pos, galley, text_color)` substitutes it at paint time. Use it for ordinary text in a custom layouter and the document follows light or dark mode for free. Anything given a literal colour does not, which is why the raw-steno red has a light and a dark shade: a single red cannot stay readable on both backgrounds, and egui defaults to dark.

**`rust-version` in the workspace manifest gates dependency resolution.** Setting it to 1.90 silently held eframe back to 0.33.3 even though 0.35.0 was available, with only a quiet "available: v0.35.0, requires Rust 1.92" note. If a crate resolves older than expected, check this before anything else.

**Cargo is not on `PATH` in fresh shells** until the user's profile is reloaded after the rustup install. Prefix commands with `$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH";` or the tool call fails with a confusing "not recognized".

---

## Output routing and the live view (M3, M5)

**Exactly one destination per output batch, never both.** Focused window decides: our document, or `SendInput` to another app. This is not a preference, it is what makes double-typing structurally impossible instead of an intermittent bug you chase forever. Any refactor that lets both paths fire has broken the core design.

**Red raw-steno ranges live in the document model, not a transient buffer.** That is what makes red survive indefinitely and disappear correctly when undone. Storing them in a render-time buffer will look identical in early testing and lose colours as soon as anything scrolls or reflows.

**egui's layouter runs at least once per frame.** Memoise the highlight computation. Recomputing colour ranges over a long document 60 times a second will be the first performance problem you meet, and it will present as vague UI sluggishness rather than anything pointing at the layouter.

---

## Project shape

**This is not a Plover fork and Plover is GPL.** Read Plover's source for semantics, never copy code. The `reference/` specs exist so you mostly do not need to open it at all.

**Do not reintroduce a machine picker as a helpful default.** The entire reason this project exists is that Plover makes the user select a machine and then gives up when it is absent. Auto mode with a forever-retry scanner is the point. A dialog, a manual reconnect button as the primary path, or any "machine not found, giving up" branch defeats the goal. (A pinned-machine override in settings is fine as an escape hatch.)

**The plan says "hardcoded to Stenograph USB" in its early framing.** That was superseded during planning: it became the Auto scanner once more protocols entered scope, which solves the same pain better. `PLAN.md` notes this, but if you see the two phrasings and they seem to conflict, Auto scanner is current.

**Test with the Peregrine, not the Luminex.** From M4a onward there is real hardware available. The Luminex needs a driver install and only enters at M8. Do not build fake input scaffolding beyond the small M3 dev box, and do not wait for the Luminex to test anything.

**The dictionary files are shared with a working Plover install.** They live in `C:\Users\Corien\AppData\Local\plover\plover\` and official Plover stays installed as the user's fallback. Never move, rename, or restructure them. Back up before the first write from Pluvialis. Editing entries in place is expected and fine.

**At M8, close official Plover first.** Two programs cannot hold the writer handle simultaneously. If the first hardware test fails mysteriously, check this before anything else.

**The name is settled.** Dotterel was the first choice and is taken by an existing Android steno app. Pluvia is heavily used. Pluvialis was checked and is clear. No renaming.

---

## House style

- **UTF-8 on every read and write.** An encoding error means the encoding is wrong; fix it, never mask it with lossy replacement.
- **No em dashes** anywhere: prose, comments, commit messages, UI strings.
- **No emoji** in code or terminal output.
- **`py` launcher** for Python helper scripts, never `python3`.
- **Warnings are failures.** `cargo clippy` clean is part of every milestone, not a cleanup pass at the end.
- **Comments state constraints, not narration.** "cbSize must be 5 here per Win32" earns its place; "open the device" does not.
