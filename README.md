# Figura Munda

**Shows you what a file reveals about you, then removes it.**

*Munda* as in clean. The adjective agrees with *figura*, following Figura Obscura.

> **Status.** The engine, the command line, the desktop window and the mobile
> runner exist and are tested: 261 tests and 103 independent fixture checks.
> What is not built, and what has not been verified, is in
> [Where it stands](#where-it-stands). Read that before trusting this with a
> document that matters.

## Contents

[Why](#why) · [Install](#install) · [Use](#use) ·
[What it finds](#what-it-finds) · [Profiles](#profiles) ·
[Never remove](#never-remove) · [What it refuses](#what-it-refuses) ·
[Format coverage](#format-coverage) · [Trust posture](#trust-posture) ·
[Where it stands](#where-it-stands) · [Layout](#layout) ·
[Further reading](#further-reading) · [Licence](#licence)

## Why

Every file you send carries more than you meant to send: where you were, what
you own, who you work for, and which of your files belong to the same person.
Most of it is commercially useful to somebody. The business model that reads it
does not need your name, only a handle that is stable across your files, which
is why the correlation identifiers in this tool's list matter more than the
obvious fields.

So this is a tool, and it is also an argument: the amount a person hands to
advertisers and recommendation systems should be a setting they control, and the
default should not be all of it. Every file cleaned is revenue that does not
happen.

It only works at scale, and scale means the tool has to be pleasant enough to
leave switched on. That is why the mobile runner exists, why the window takes
folders rather than one file at a time, and why nothing here needs an account.

**Inspection is the product; cleaning is the side effect.** A scrubbed file
looks identical to the original, so a user has no reason to believe anything
happened. Four visible artefacts answer that: the report, the diff, verify mode,
and the recipient view. The reasoning is in
[docs/design.md](docs/design.md).

## Install

Rust 1.82 or newer. No network at build time beyond crates.io, and none at all
at run time.

```sh
cargo build --release     # target/release/fm-cli and target/release/fm-gui
```

The window additionally needs `libwayland-dev`, `libxkbcommon-dev` and the X11
headers on Linux. `ffmpeg` is needed only to generate the test fixtures.

## Use

```sh
fm-cli inspect photo.jpg                 # what is in it; writes nothing
fm-cli inspect ~/Pictures -r --recipient # a folder, and what a recipient sees
fm-cli clean photo.jpg --out ~/sent      # clean a copy, leaving the original
fm-cli clean ~/Pictures -r -p strict     # a folder, under a stricter profile
fm-cli clean report.pdf --dry-run        # the whole diff, writing nothing
fm-cli verify ~/sent -r                  # assert the output really is clean
fm-cli profile strict --save mine.toml   # a profile to edit and carry
fm-gui                                   # the window
```

Three things it does rather than surprise you:

- **`inspect` never writes.** Neither does `clean --dry-run`.
- **Removing something a reader can see waits for consent.** An uncropped
  original, an off-canvas paragraph or a comment thread is listed in full, and
  `clean` stops until you pass `--allow-content-loss`.
- **A clean is refused rather than fudged.** See
  [What it refuses](#what-it-refuses).

The window runs the same engine in two phases, look then clean, and cannot reach
the second without the first. The phone runs a background sweep over the folders
you grant it, under a profile the desktop wrote. Both are described in
[docs/design.md](docs/design.md), and the mobile side in detail in
[mobile/README.md](mobile/README.md).

## What it finds

Ten classes. A profile decides what happens to each.

| Class | Examples |
|---|---|
| **Identity** | Author, owner, last-modified-by, company, registered organisation, creator account |
| **Device** | Camera make and model, body and lens serials, maker notes, scanner model, OS and software version, PDF producer string |
| **Location** | GPS position, altitude, heading, place names, timezone offset |
| **Time** | In-file created, modified, printed, EXIF original and digitised, PDF `CreationDate` and `ModDate` |
| **Correlation handles** | XMP `DocumentID`, `InstanceID`, `OriginalDocumentID`, PDF `/ID` array, OOXML `rsid` save IDs and `docId` GUIDs. The nastiest class: they link two files to one author or session even with every name stripped. |
| **History** | Tracked changes, comments, prior authors, fast-save remnants, and the PDF incremental-update chain, from which earlier versions are recoverable in full |
| **Hidden payload** | Embedded thumbnails holding the uncropped original, preview images, attached files, PDF JavaScript, hidden layers, rows, slides and speaker notes, off-canvas SVG content |
| **Redaction integrity** | Selectable text under filled shapes, vector art under placed images, an OCR layer under a scanned redaction |
| **Structural fingerprint** | ZIP entry order and timestamps in OOXML, PDF object ordering and xref style, compression choices. Identifies the writing software and often its version, narrowing authorship even when all fields are gone. |
| **Filesystem** | mtime, atime, birthtime, extended attributes, macOS `com.apple.metadata`, Windows `Zone.Identifier` (which records that the file was downloaded, and from where) |

## Profiles

Profile choice is the single decision a user makes.

| Class | `standard` | `strict` | `uniform` |
|---|---|---|---|
| Identity | remove | remove | remove |
| Device | remove | remove | remove |
| Location | remove | remove | remove |
| Time | keep | coarsen to day | normalise to constant |
| Correlation handles | remove | remove | remove |
| History | remove | remove | remove |
| Hidden payload | remove | remove | remove |
| Redaction integrity | report only | enforce | enforce |
| Structural fingerprint | keep | keep | rewrite uniformly |
| Filesystem | keep | reset | normalise to constant |

There is a fourth, `inspect`, which keeps everything and is how the tool reports
without touching anything. `custom` overrides any cell. Profiles are TOML, they
round-trip, and the file the desktop writes is the file the phone reads. Cells
are validated rather than trusted: `identity = "coarsen-day"` is rejected at
load, because coarsening a name means nothing and a profile that silently did
nothing would be worse than one that refused.

The full file, and why `uniform` exists at all, are in
[docs/design.md](docs/design.md).

## Never remove

Stripping any of these is a regression rather than a protection, so no profile
can:

- **ICC colour profiles.** The image changes appearance without them.
- **Tagged-PDF structure.** It is what a screen reader follows through a
  document.
- **EXIF orientation.** Removing it turns photographs on their side. This one
  was found by building the thing, not by planning it.
- **SVG `<title>` and `<desc>`.** What a screen reader reads out, kept even
  though they can name an author.
- **Anything inside a signed PDF**, which is detected and refused rather than
  silently invalidated.

Each is reported as kept on purpose, with the reason, rather than quietly left
in. Two more things are reported and never acted on, because acting on them is a
decision about the document rather than about its metadata: **hidden
worksheets** and **speaker notes**. Deleting a sheet is the user's call, and the
report says so in those words.

## What it refuses

A refusal is a feature here, not a shortfall. Every one of these was a way to
lose a file, break it, or be told something false about it, so each is a refusal
naming the path and the reason rather than a warning to be clicked past.

**Files it will not touch:**

- A **signed** PDF: any metadata edit invalidates the signature.
- An **encrypted** PDF: the text and metadata are ciphertext and the trailer
  identifier is part of the key, so a rewrite leaves a file nothing opens.
- An **OpenDocument** package, until the vocabulary is read. Read with the OOXML
  part names it has no properties at all, which in a report is indistinguishable
  from a document that has none.
- An **incomplete copy**: an MP4 whose boxes do not account for every byte. The
  fields are still in it, unreachable, so a field count would be a lie.
- A PDF **nested deeper than the parser survives**, counted before parsing,
  because the alternative is the process aborting mid-folder.

**Writes it will not make:**

- **Over the original**, which `--out` pointed at the originals' own folder used
  to do silently. `--in-place` is how you say you mean it.
- **Over a file that is not this file cleaned.** Running the same job twice
  writes again, because cleaning is idempotent; anything else is somebody's
  file, and `--overwrite` is how you say otherwise.
- **Two sources with one name into one folder**, which would keep whichever
  finished last. The whole list is checked before anything is written.
- **Through a symlink, or over one name of a hardlinked file, in place.** The
  first leaves the original dirty where the link pointed; the second leaves it
  dirty under every other name.
- **Anything at all** when the diff names a field the report did not, or when
  the output fails its own verification.

## Format coverage

| Format | State |
|---|---|
| **JPEG** | Done. EXIF in every IFD, IPTC in the Photoshop resource block, XMP, the ICC profile, comments, and the IFD1 thumbnail that holds the uncropped original. |
| **PNG** | Done. `tEXt`, `zTXt`, `iTXt` including XMP, `tIME`, `eXIf`, with `iCCP` and the colour chunks untouched. An `eXIf` block is unusual enough in a PNG that blending drops the whole thing, except for what the never-remove rule keeps: the orientation and a colour profile stored in it are written back, and the report names the crowd that leaves. |
| **SVG** | Done. The metadata block, the editor namespaces, markup comments, scripts, and content parked outside the `viewBox`, judged by what an element covers rather than where its anchor sits, so a bleed background is artwork and not a payload. |
| **OOXML** | Done for DOCX. Core and app properties, custom properties, the thumbnail, comments, the people list, tracked changes, `rsid` save identifiers, the document GUID, and the container's own member order and timestamps. XLSX and PPTX share the engine; their format-specific parts (hidden sheets, speaker notes) are reported, not removed. |
| **PDF** | Done. Info dictionary, XMP, the trailer `/ID`, attachments and JavaScript wherever they hang, in the catalog name tree and on a page's own annotations and actions alike, incremental-update history, the redaction check, and signature detection. An encrypted document is refused rather than rewritten, and so is one nested deeper than the parser survives, which is counted before the file is parsed. |
| **MP4 / MOV** | Done. `udta` and `ilst`, including the `keys`-indexed `ilst` a recent iPhone writes, the 3GPP `loci` box, the `©xyz` position a handset writes, and the header timestamps, with the sample tables corrected for what was removed. A copy whose boxes do not account for every byte is refused rather than part-read. |
| **ODF** | Not yet, and refused by name rather than part-read. The ZIP and XML layers are in place and shared; the part names and the metadata vocabulary are not, and reading an ODF package with the OOXML part names finds no properties at all, which in a report is indistinguishable from a document that has none. `report.odt` is a fixture of exactly that, declaring the fields the engine does not read. |
| **HEIC, TIFF** | Not yet. The EXIF reader and writer are shared, so TIFF is mostly container work; HEIC needs the ISOBMFF item structure, which the MP4 box walker is a start on. |
## Trust posture

No network, ever. No accounts, no telemetry, no model downloads. Input and
output both sit on disk and can be diffed by anyone who doubts the claim.
Reproducible builds, so a published binary can be checked against this source.

Three of those are mechanically checkable, so they are checked rather than
asserted:

```
tools/check-trust.sh          networking crates, socket symbols, licences
tools/check-reproducible.sh   the same source in two paths, byte for byte
```

Both run on every push, in
[`.github/workflows/checks.yml`](.github/workflows/checks.yml), alongside the
suite, the fixture contract and the sign-off this project asks for. What they
find:

- **No dependency in any of the four closures can do networking.** 426 crates,
  nothing that speaks a network protocol or resolves a name.
- **`fm-cli` and `libfm_runner.so` import no socket symbols at all**, so those
  artefacts cannot open one: a property of the file rather than a promise about
  the source. **`fm-gui` imports four**, and the honest reason is that a display
  server is reached over a Unix socket and the X11 client in the clipboard stack
  can be pointed at a remote display. Anyone who wants the version that is
  provable from the binary alone should use the command line.
- **No dependency is under a licence with no permissive alternative**, which is
  what a separately-licensed layer linking this engine requires.
- **The build is reproducible**, and was not until the build path was remapped:
  the same source in two directories gave two different binaries, because build
  scripts embed their `OUT_DIR`. The remapping is part of the release recipe,
  not an optimisation.

A check that can pass without reading anything is not a check, which these
could: the symbol check skipped quietly when the binaries were absent, and the
reproducible one varied only the output directory. Both now fail instead.

## Where it stands

**Tested, and the tests say what they test.** 261 tests, plus 103 checks in
`fixtures/verify.py` that re-read the generated files sharing no parsing code
with the generator. `fixtures/manifest.json` declares field by field what each
fixture holds, so the manifest is the contract between the Python side and the
Rust side and neither can quietly agree with itself. Cleaned MP4s are checked
with `ffprobe` and cleaned PDFs with `fixtures/checkpdf.py`, a demuxer and a
parser that know nothing about this engine, which is the only kind of check
that says anything about output.

**A cold review found that a green suite was worth less than it looked.** Three
readers with no stake in it being right turned up files that reported clean with
every field still in them, a PNG that lost the orientation deciding which way up
it is shown while the count called it preserved, a PDF that aborted the process
and took a folder run with it, an encrypted document cleaned into one no reader
would open, four ways for a clean to overwrite something, a diff that could name
a field the report never mentioned, and checkers that passed without reading
anything.

All of it is fixed. Each defect was reproduced with the command line before it
was touched, and each is now carried by a fixture or a test that fails if it
returns, which is why the fixture set looks the way it does: `nested.pdf`,
`encrypted.pdf`, `clip-truncated.mp4`, `clip-ios.mov`, `report.odt` and
`photo-oriented.png` are traps the engine walked into. `photo-codepage.jpg`
came from the first time the window was put on a screen, where every accented
letter arrived as a box.

**Not verified, and it matters:**

- **No PDF has been opened in a real reader.** `fixtures/checkpdf.py` parses the
  cross-reference table by hand, shares no code with the engine, checks both
  directions between the table and the objects, reports encryption and counts
  revisions. It does not render a page, so "opens in Acrobat and looks right" is
  nobody's evidence yet. Twice the checker has been the thing at fault rather
  than the engine, which is why `verify.py` now hands it three forgeries: a
  checker that cannot fail is not a check.
- **The window has been on screen once**, on a tiling compositor, which found
  two defects at once: columns that did not give way at narrow widths, and every
  accented letter drawn as tofu. Both are fixed, the second in the engine rather
  than the window. Nothing beyond those two findings has been looked at by
  anyone, and no screenshot exists.
- **Nothing in `mobile/android/` has been compiled.** No SDK, no NDK, no device.
  The review found a call in it passing a boolean where the boundary takes an
  integer, so the version published for weeks could not have built. Read it as a
  design worked out in code, not as software.
- **XLSX and PPTX** go through the OOXML handler with no fixture of their own, so
  only the parts they share with DOCX are exercised.

## Layout

```
desktop/fm-core    the engine: classes, profiles, format handlers, reports
desktop/fm-cli     the command line: inspect, clean, verify, profile
desktop/fm-gui     the window
mobile/runner      the periodic sweep, and the Android bridge
mobile/android     the Android host
fixtures/          generated test files and an independent verifier
docs/              the reasoning: design notes and the compliance position
tools/             the trust-posture and reproducible-build checks
```

```sh
cargo test                      # everything
python3 fixtures/generate.py    # the files the engine is checked against
python3 fixtures/verify.py      # re-read them, sharing no code with the generator
tools/check-trust.sh            # the trust posture, checked rather than asserted
tools/check-reproducible.sh     # two paths, one binary
```

## Further reading

| Document | What is in it |
|---|---|
| [docs/design.md](docs/design.md) | Why the options are shaped this way: the four artefacts, blending and anonymity sets, the profile file, the window, and the open decisions |
| [docs/compliance.md](docs/compliance.md) | What this is legally, what the operator is responsible for, and the policy layer that does not exist yet |
| [mobile/README.md](mobile/README.md) | The runner, the Android host, and why iOS cannot have the sweep |
| [fixtures/README.md](fixtures/README.md) | The fixture set, the stubs that are stubs, and what the independent checks have caught |
| [CONTRIBUTING.md](CONTRIBUTING.md) | MPL-2.0, sign-off, and the two habits this codebase has |

## Licence

**MPL-2.0.** File-level copyleft means nobody can close a fork of the cleaning
engine, and contributions need no CLA. The commercial layer lives in a separate
private repository that links this engine rather than absorbing it, and what it
may contain is organisation-shaped and never security-relevant: fleet policy,
dashboards, audit trails, multi-user management. Inspection, the cleaning
engine, the redaction checks and the runner never will be. The reasoning, and
how the boundary moved outward while this was built, are in
[CONTRIBUTING.md](CONTRIBUTING.md).
