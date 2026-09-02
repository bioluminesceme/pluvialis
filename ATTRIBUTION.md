# Attribution

Pluvialis is licensed **GPL-3.0-or-later** (see `LICENSE`).

It is **not a fork or a port of Plover**, but it is **not wholly independent of
it either**, and the difference matters enough to write down precisely. Two
pieces of material come from Plover, which is
[GPL-2.0-or-later](https://github.com/openstenoproject/plover), copyright
Joshua Harlan Lifton and the Open Steno Project contributors.

Because Plover is licensed "version 2 **or later**", that material may be used
under GPL-3, which is why Pluvialis can be GPL-3.0-or-later and still satisfy
the terms it inherits.

## What comes from Plover

**`crates/pluvialis-core/assets/american_english_words.txt`** is a byte-for-byte
copy of `plover/assets/american_english_words.txt` (338,882 lines, 4.8 MB). It is
embedded in the executable and used to choose between candidate spellings when a
suffix attaches to a word.

**`crates/pluvialis-core/src/orthography_rules.rs`** is the 38 orthography rules
and their aliases from `plover/system/english_stenotype.py`, mechanically
converted from Python to Rust regex syntax. The selection and arrangement of
these rules is Plover's work, not ours; only the syntax conversion is ours.

Both are the same category of thing: the English-language data that makes
`{^ing}` produce "running" rather than "runing". Reimplementing them from scratch
would mean inventing a different set of rules and getting different output, which
is precisely what a user switching from Plover does not want.

## What does not come from Plover

Everything else is independently written. Plover's source was read as a
specification for protocols and behaviour, and the following are **facts and
interfaces rather than expression**: the Stenograph USB wire format, the Gemini
PR packet layout, the steno key chart, the meta-command syntax, the RTF/CRE
dictionary format, and the Python dictionary calling convention. Pluvialis
implements these to interoperate with the same hardware and the same dictionary
files.

Several parts deliberately behave *differently* from Plover, including the
machine scanner (which retries forever instead of giving up), the transport's
handle handling, the output router, the document model, and the dictionary
library. Some of these exist specifically because Plover's behaviour was the
problem being solved.

## The icon

**`crates/pluvialis-app/assets/icon-source.png`** is a flat illustration of a
golden plover, generated with ChatGPT by the author of this project. No third
party holds copyright in it, so it ships under the project's own licence like
everything else here.

`icon.png` and `icon.ico` are built from it by `tools/make_icon.py`, which cuts
the white page away, keeps the rounded corners, and stores a different crop at
each icon size so the small ones stay readable. Run that script after changing
the source art, then rebuild, since `build.rs` embeds `icon.ico` into the
executable.

Nothing about the icon comes from Plover.

## Why not call it "a Rust port of Plover"

It was considered and rejected on 2026-07-20, on accuracy grounds in both
directions. It claims more derivation than exists: the translator, formatter,
machine layer, document model and storage are independently designed, and several
differ from Plover on purpose. It would also attribute this program's bugs to
Plover's authors, who did not write them.

"An independent steno program that incorporates Plover's English orthography
data" is longer and true.

## If you add to this project

Copying more from Plover is allowed by the licence and must be recorded here when
it happens. What is not allowed is copying and *not* recording it: the accuracy of
this file is the only thing keeping the licence claim honest.
