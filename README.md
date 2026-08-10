# Pluvialis

A fast stenography program that automatically detects connected keyboards or writers.
Based on Plover, written in Rust.
It runs on Windows only.

Pluvialis translates chords from a steno writer, either into its own editor or into whatever other application has focus.

![The Pluvialis window: a top bar with Open, Save and Save As, an editor showing live steno text, a stroke tape on the right listing each outline and what it produced, and a status bar showing the connected writer, word count, and speed.](docs/screenshot.png)

## How to install

Click on the Releases in the right sidebar here on Github and you will find the  Zip under Assets.
Unzip to a separate folder, for example C:\Pluvialis , and run the exe. Windows will likely warn you.
The .exe opens the program right away, there is no installation process. You can pin the icon to your taskbar to start it easier.

## Why it exists

Plover is excellent, but its Stenograph plugin cannot recover when the writer is
not present at the moment capture starts. If the machine is off, asleep, or
plugged in a second too late, the fix is to restart Plover and reselect the
machine, or go back and forth and try to auto detect the port until it finally works. 

Pluvialis exists to remove this annoying ritual: it scans for a
writer continuously and connects to it the moment one appears, so you can start typing right away.

Pluvialis owes a great deal to [Plover](https://github.com/openstenoproject/plover),
the open-source stenography engine by the Open Steno Project. It is best understood
as a partial reimplementation of Plover in Rust, with some features left out and
some added. It is not literally a fork: the code is written fresh rather than
branched from Plover's, and a few things behave deliberately differently. But it 
was built by reading Plover's source as the specification
for every protocol and format. See [ATTRIBUTION.md](ATTRIBUTION.md) for exactly
what comes from where, and the [Acknowledgements](#acknowledgements) below.

The name also heavily links to the original Plover, since the Latin *Pluvialis* is the golden-plover genus.

## Status

Early but working well for me. It autoconnects to both my Luminex CSE and a Peregrine, 
loads both my dictionaries, and renders unknown chords in red in the live view. 
And it's super fast. The translation core loads 101,407 dictionary entries in about 50 ms and answers lookups in microseconds.

This is version 0.1.0 and the interface is still moving. It is developed against
one person's daily setup, so paths and assumptions elsewhere in the tree may reflect
that.


## Features

- **Continuous auto-connect.** No dialog, no machine picker. Pluvialis keeps
  scanning and connects when a supported writer appears, including after an
  unplug or a power cycle.
- **Live editor with a caret.** For quick notes I wanted Pluvialis to have its own window to type in,
  with the tape on the right side. 
- **Untranslated chords shown in red**, attached to the characters they belong
  to, so they survive edits until undone. Saw this on YouTube, looked useful.
- **Type into other applications.** if this is enabled, Pluvialis forwards the translated strokes to whichever window has focus.
- **Open / Save / Save As**, with autosave, timestamped version history, and crash recovery of the markdown file (you can save this when you are typing in the Pluvialis live typing window)
- **A live words-per-minute meter** take it as a rough estimate, I have no idea how accurate this is yet.
- **Bring your own dictionaries.** Import Plover JSON and Python dictionaries,
  then enable, disable, reorder by priority, and look up outlines or words in the
  Dictionaries pane. Which dictionaries are enabled is remembered between runs.
  You can change, delete and append dictionary entries.
- **Command-line tools** for looking up outlines, checking dictionaries, and
  cleaning invalid entries.

## Machine support

Pluvialis auto-detects two protocol families. It tries them in this order:

1. **Stenograph USB** (Windows only): the Luminex CSE and relatives. You need to have the Stenograph USB drivers installed.
2. **Gemini PR** over USB serial: the Peregrine, and other keyboards or serial devices that speak Gemini PR. (Should support a lot of other steno boards)

If both Stenograph and Gemini PR are attached, the Stenograph writer is preferred. 
Other steno protocols (TX Bolt, Stentura, Passport, ProCAT, plain NKRO keyboard, and so on) are not (yet) implemented.

## Dictionaries

A fresh Pluvialis starts with **no dictionaries**. You add your own, and
Pluvialis copies each into `dictionaries\` and owns that copy. It will let you delete/change/add entries. 
The original you imported from is left untouched. 

Add a dictionary with the **Add dictionary** button in the Dictionaries pane, or
from the command line with `pluvialis import <FILE>`. Both accept Plover `.json`
and `.py` dictionaries and refuse anything else.

- **JSON dictionaries** (RTF/CRE format) load enabled, in priority order.
- **Python dictionaries** (Plover's `.py` dictionaries, such as jeff-phrasing)
  run as written through an embedded CPython interpreter. They are discovered
  and loaded **disabled**, appearing with a checkbox you tick to enable. A
  Python dictionary is arbitrary code with no sandbox, the same trust model as
  Plover.

**CPython 3.12 or newer must be installed** and findable on the DLL search path.
This is not only for Python dictionaries: the executable imports `python3.dll`
at load time, so without CPython present Windows refuses to start Pluvialis at
all. It links the stable ABI, so any 3.12+ install works; it is not tied to one
version.


## Building

Requires the Rust toolchain (edition 2024, Rust 1.97 or newer) with the MSVC
toolchain on Windows, and a CPython 3.12+ install for the Python dictionary
support.

```
cargo build --release
```

The application binary is `target\release\pluvialis-app.exe`.

Run the tests and the linter (both must be clean):

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Running

With no arguments, the executable opens the GUI:

```
target\release\pluvialis-app.exe
```

It also has command-line subcommands, useful for diagnosing dictionaries and the
machine layer without the GUI:

- `lookup <OUTLINE>...` answers from the real dictionaries, with timings.
- `check [DICT...]` formats every entry and reports unimplemented meta commands.
  It exits non-zero on anything new, so it doubles as a regression check.
- `clean [--write] [DICT...]` removes entries whose keys are not valid steno. It
  is a dry run unless `--write` is given, and it keeps the original alongside.
- `machine [SECONDS]` runs the Auto scanner and prints every status change and
  stroke. Set `RUST_LOG=pluvialis_machine=trace` for per-attempt detail.

## Architecture

A Cargo workspace of five crates, split so that OS-specific and
machine-specific code stays quarantined:

| Crate               | Responsibility                                                                    |
| ------------------- | --------------------------------------------------------------------------------- |
| `pluvialis-core`    | Strokes, keymaps, dictionary load and lookup, translator, formatter, undo history |
| `pluvialis-machine` | The `Machine` trait, the Auto scanner, and each protocol                          |
| `pluvialis-output`  | Keystroke emulation into other applications (Win32 `SendInput`)                   |
| `pluvialis-python`  | Plover's Python dictionaries, run through embedded CPython                        |
| `pluvialis-app`     | The egui/eframe GUI, documents, autosave, versioning, and config                  |

## Acknowledgements

Pluvialis would not exist without [Plover](https://github.com/openstenoproject/plover),
the open-source stenography engine by Joshua Harlan Lifton and the Open Steno
Project contributors. Plover made open-source steno real, and this project is best
understood as a partial reimplementation of it in Rust, with some features removed
and some added:

- Its American English word list and its 38 orthography rules are taken directly
  from Plover, so suffixes and spelling behave exactly as a Plover user expects
  (see [ATTRIBUTION.md](ATTRIBUTION.md)).
- Plover's source was the specification for everything Pluvialis interoperates
  with: the Stenograph and Gemini PR protocols, the steno key chart, the RTF/CRE
  and Python dictionary formats, and the meta-command conventions.
- Plover's design, and in a few places the specific problems this project set out
  to solve, shaped how it works.

Pluvialis is written independently rather than forked, but that speaks only to
where the code came from, not to how much it owes Plover.
If you want mature, cross-platform stenography that supports far more hardware,
use Plover. It is excellent, and Pluvialis stands on its shoulders.
If you want something fast on Windows, that auto detects your machine, try this.

## Licence

**GPL-3.0-or-later.** See [LICENSE](LICENSE) for the full text and
[ATTRIBUTION.md](ATTRIBUTION.md) for what is derived from Plover and why the
licence follows. Plover is GPL-2.0-or-later, and the "or later" makes GPL-3
compatible with the material Pluvialis embeds.
