# Figura Munda

Shows you what a file reveals about you, then removes it.

*Munda* as in clean. The adjective agrees with *figura*, following Figura Obscura.

> **Status:** the engine, the command line, the desktop window and the mobile
> runner exist and are tested. What is not built, and what has not been
> verified, is listed under [Where it stands](#where-it-stands). Read that
> before trusting anything here with a document that matters.

## Why

Every file you send carries more than you meant to send. Most of it is
commercially useful to somebody: where you were, what you own, who you work for,
and which of your files belong to the same person. The business model that reads
it does not need your name, only a handle that is stable across your files, which
is why the correlation identifiers in this tool's list matter more than the
obvious fields.

So this is a tool, and it is also an argument: that the amount of information a
person hands to advertisers and recommendation systems should be a setting they
control, and that the default should not be "all of it". Every file cleaned is
revenue that does not happen. That is the point, and the other Figura projects
are the same argument applied elsewhere.

It only works at scale, and scale means the tool has to be pleasant enough to
leave switched on. That is why the mobile runner exists, why the desktop window
takes files and folders rather than one file at a time, and why nothing here
needs an account.

## The problem this has to solve first

The cleaning is invisible. A scrubbed file looks identical to the original, so the
tool does its real work as an unseen hand and a user has no reason to believe it
worked, or that it was ever needed. That is the product risk, not the file formats.

The answer is that **inspection is the product and cleaning is the side effect.**
Four visible artefacts carry it:

1. **The report.** Plain language, not a field dump. "This photo records where it was
   taken, which phone took it, and still contains the uncropped original."
2. **The diff.** Before and after, field by field, with what was kept and why.
3. **Verify mode.** Re-inspect the output and assert it is clean. Also the regression
   test suite, for free. It checks values rather than absence, because a coarsened
   timestamp is supposed to still be there, and that is what catches a handler
   which wrote the original value back out. It has a third answer besides pass
   and fail: a PDF trailer identifier that was replaced and one that was never
   touched are indistinguishable in the finished file, so checking a file on its
   own reports that as unsettled rather than as a failure, and checking it against
   the diff settles it.
4. **Recipient view.** Render only what survives, so the user sees what the other
   side sees.

A count helps too: "47 fields across 6 classes, 41 removed, 6 preserved."

## The option model

This is the core of the design. Two axes, rolled up into selectable profiles.

### What is in a file

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

### What can be done to each

`keep` · `remove` · `replace` with a generic constant · `coarsen` (timestamps to the
day or month) · `normalise` to a fixed constant across all files

Three findings that shape the defaults:

- **Normalising beats falsifying.** A plausible fake timestamp is worse than a removed
  one. It can matter legally if the document is evidentiary or goes to an authority,
  and it will contradict whichever fields survive, which is itself a signal. A fixed
  constant, the way reproducible builds pin ZIP entries to 1980-01-01, is uniform and
  makes no false claim.
- **Coarsening GPS is usually worse than removing it.** One decimal place still puts
  you in a named neighbourhood while implying precision was handled.
- **Stripping is itself a fingerprint.** A file cleaned in exactly `mat2`'s pattern
  identifies you as a `mat2` user. Where there is a choice, match what a common tool
  or platform already emits, so the output joins a large anonymity set rather than a
  small one.

### Blending in

The last of those three is a behaviour, not only a principle, and it is the second
switch a profile carries: **match common tools** or **strip to the minimum**.

Matching usually wins. A JPEG with no JFIF header at all leaks fewer bytes and
marks you as someone who runs a metadata stripper, which is a smaller crowd to
hide in than "every image a server ever resized". So the default keeps the inert
headers that ordinary software leaves, and the engine names the crowd the output
joined: *"looks like a plain baseline JPEG with no application segments (very
common)"*.

Two consequences worth stating, because both surprised us while building it:

- **A field can be removed that the profile said to keep.** A date in a PNG or a
  `udta` date in an MP4 is written by almost nothing except the device that made
  the file, so keeping one is the fingerprint. Those are dropped under `match
  common tools` even when `Time` is set to keep, and the diff says exactly that
  and why. A date in a JPEG's EXIF or a PDF's Info dictionary is ordinary, so
  those are left to the profile. The question is never "does this format have
  dates" but "is a date normal *here*".
- **Keeping any date costs you the largest crowd.** It is a real trade and the
  tool reports it rather than hiding it: `standard` on a photograph lands in "a
  JPEG keeping only its capture date", which is common but not the most common.
  Removing the date reaches "an EXIF block holding orientation and nothing else",
  which is what every resize pipeline leaves.

Every entry in the anonymity-set table carries its **provenance**: whether it
follows from the format specification, was measured on a real file, or is simply
assumed. Most are still assumed, and reports say so in those words. The leakage
survey below is what turns assumptions into measurements, and that table is the
slot its results land in. Nothing in the engine depends on an entry being right:
a wrong one produces a wrong sentence in a report, never a leak.

### Profiles

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
without touching anything.

`uniform` is the one that makes two files from different people indistinguishable,
and it is also the one most likely to look odd to a recipient. `custom` overrides any
cell. Profile choice is the single decision a user makes.

Profiles are TOML, they round-trip, and the file the desktop writes is the file
the phone reads:

```toml
name = "strict"
description = "Everything in standard, plus dates coarsened to the day, ..."
blending = "match-common"

[actions]
identity = "remove"
device = "remove"
location = "remove"
time = "coarsen-day"
correlation = "remove"
history = "remove"
hidden-payload = "remove"
redaction-integrity = "enforce"
structural-fingerprint = "keep"
filesystem = "reset"

[constants]
generic-text = ""

[constants.timestamp]
year = 1980
month = 1
day = 1
hour = 0
minute = 0
second = 0

[runner]
enabled = true
interval-hours = 6
roots = ["DCIM/Camera"]
new-files-only = true
keep-original = false
```

(`fm profile strict --save mine.toml` writes exactly that, and `fm clean
--profile mine.toml` reads it back.)

Cells are validated rather than trusted: `identity = "coarsen-day"` is rejected
at load, because coarsening a name means nothing and a profile that silently did
nothing would be worse than one that refused.

### Never remove

Stripping these is a regression, not a protection: ICC colour profiles (the image
changes appearance), tagged-PDF accessibility structure, and anything inside a
**signed** PDF. Detect signatures and refuse rather than silently invalidating them.

Building it added two more. **EXIF orientation** decides which way up a photograph
is shown, so removing it turns pictures on their side; it is reported as kept on
purpose, like the colour profile. **SVG `<title>` and `<desc>`** are what a screen
reader reads out, so they stay even though they can name an author, and the report
says they were kept and why.

And two things are reported but never acted on, because acting on them is a
decision about the document rather than about its metadata: **hidden worksheets**
and **speaker notes**. Deleting a sheet is the user's call. The report says so in
those words rather than quietly leaving them out.

## Coverage

| Format | State |
|---|---|
| **JPEG** | Done. EXIF in every IFD, IPTC in the Photoshop resource block, XMP, the ICC profile, comments, and the IFD1 thumbnail that holds the uncropped original. |
| **PNG** | Done. `tEXt`, `zTXt`, `iTXt` including XMP, `tIME`, `eXIf`, with `iCCP` and the colour chunks untouched. |
| **SVG** | Done. The metadata block, the editor namespaces, markup comments, scripts, and content parked outside the `viewBox`. |
| **OOXML** | Done for DOCX. Core and app properties, custom properties, the thumbnail, comments, the people list, tracked changes, `rsid` save identifiers, the document GUID, and the container's own member order and timestamps. XLSX and PPTX share the engine; their format-specific parts (hidden sheets, speaker notes) are reported, not removed. |
| **PDF** | Done. Info dictionary, XMP, the trailer `/ID`, attachments, JavaScript, incremental-update history, the redaction check, and signature detection. |
| **MP4 / MOV** | Done. `udta` and `ilst`, the 3GPP `loci` box, the `©xyz` position a handset writes, and the header timestamps, with the sample tables corrected for what was removed. |
| **ODF** | Not yet. The ZIP and XML layers are in place and shared; the part names and the metadata vocabulary are not. |
| **HEIC, TIFF** | Not yet. The EXIF reader and writer are shared, so TIFF is mostly container work; HEIC needs the ISOBMFF item structure, which the MP4 box walker is a start on. |

## Trust posture

No network, ever. No accounts, no telemetry, no model downloads. Input and output
both sit on disk and can be diffed by anyone who doubts the claim. Reproducible
builds so a published binary can be checked against this source.

## Deliverables

```
desktop/fm-core    the engine: classes, profiles, format handlers, reports
desktop/fm-cli     fm inspect | clean | verify | profile
desktop/fm-gui     the window
mobile/runner      the periodic sweep, and the Android bridge
mobile/android     the Android host
fixtures/          generated test files and an independent verifier
```

```
cargo test                     # everything, 215 tests
cargo run -p fm-cli -- inspect ~/Pictures
cargo run -p fm-gui
python3 fixtures/generate.py   # the test files the engine is checked against
```

### The window

Two phases, in that order: **look at the files, then clean them.** The first
phase writes nothing. The second cannot be reached without passing through the
first, and changing the profile or adding a file sends the session back, because
a review of a different profile is not a review of this one.

- Files or whole folders, by button or by dropping them on the window.
- The four artefacts as tabs on whichever file is selected: what is in it, what
  would change, what the recipient would see, and the profile table that produced
  all three.
- The file list opens on the file with the worst thing in it, and colours each
  entry by what that thing is.
- Any change that would remove something a reader can see is listed in full and
  waits for a tick. Removing a comment thread, an off-canvas paragraph or an
  uncropped original should be a decision, not a surprise.
- Where the cleaned files go is a choice: beside the originals, into a folder, or
  replacing them. Replacing is possible and says plainly that there will be
  nothing left to compare against.

All of that logic lives in a `Session` with no GUI code in it, so the flow is
tested without a display: 22 tests covering the state machine, the gate and the
three destinations.

### The phone

A photograph is geotagged the moment it is taken, and a user who has to remember
to clean it before sending it will forget. So the phone's product is a
**background runner**: it sweeps the folders it was pointed at, on a schedule,
under one profile, and says what it did. The profile file is the one the desktop
writes, unchanged, so one profile can be carried between them.

The runner has no file paths in it, which is the decision the mobile design turns
on: since scoped storage an Android app reaches the camera roll through document
URIs, not paths. The reasoning, the JNI surface, and why **iOS cannot have the
runner at all**, are in [`mobile/README.md`](mobile/README.md).

### Shell integration

Not built. Explorer, Nautilus and Dolphin entries and macOS Quick Actions all
shell out to `fm clean`, so they are packaging rather than engineering, and they
follow the path already proven by Figura Obscura.

**No browser extension.** Transparent upload interception is not buildable: MV3's
`declarativeNetRequest` cannot touch request bodies, blocking `webRequest` is gone,
and `requestBody` is read-only even in Firefox. The MAIN-world JS-patching workaround
breaks on every site redesign, which is a maintenance tax this project exists to
avoid.

## Licence

**MPL-2.0**, with the commercial layer kept in a separate private repository that
links this engine rather than absorbing it. File-level copyleft means nobody can
close a fork of the cleaning engine, and contributions need no CLA. See
[CONTRIBUTING.md](CONTRIBUTING.md).

**The boundary moved while this was built, and it moved outward.** The original
note put batch processing and watched folders on the closed side. They are now
open: a tool that cleans one file at a time is a tool nobody leaves switched on,
and a mobile runner that is not free is a movement that does not spread. Folder
runs, profiles, the window and the periodic sweep are all MPL.

What remains available to close is organisation-shaped and never
security-relevant: distributing a policy to a fleet, dashboards over many
machines, compliance audit trails, and multi-user management. Inspection, the
cleaning engine, the redaction checks and the runner never will be.

## Where it stands

**Tested, and the tests say what they test.** 215 of them. The engine is checked
against `fixtures/manifest.json`, which declares field by field what each
generated fixture contains, so the manifest is the contract between the Python
generator and the Rust engine and neither can quietly agree with itself. The
cleaned MP4 is checked with `ffprobe`, which is a demuxer this project did not
write.

**Not verified, and it matters:**

- **No PDF has been opened in a real reader.** The output reloads and re-inspects,
  but there is no second implementation here to check it against, so "still a
  valid PDF" rests on one library's opinion. An independent structural check in
  `fixtures/verify.py` would close most of this; a real reader would close the
  rest.
- **The window has never been on screen.** It was built where there is no display
  and no Xvfb. It links and runs as far as creating a window. Nobody should call
  the layout finished until it has been seen.
- **Nothing in `mobile/android/` has been compiled.** No SDK, no NDK. It is the
  interface worked out in full and labelled unverified in every file.
- **XLSX and PPTX** go through the OOXML handler but have had no fixture of their
  own, so only the parts they share with DOCX are exercised.

## Open decisions

1. **Leakage survey.** Which destinations actually leak? Major social platforms
   re-encode and strip EXIF anyway, so the real exposure is webmail attachments,
   forums, file hosts and application portals. This also decides most of the
   anonymity-set table, which is currently honest about being mostly assumption.
2. **Whether to write a generic `/Producer` into cleaned PDFs.** Almost every PDF
   in circulation names a producer, so removing it is conspicuous, and the tool
   currently removes it and says the result is distinctive. Writing "Figura
   Munda" is true and identifies the tool. Writing a common vendor string is
   neither. A profile can already do it via `replace`; what is undecided is the
   default, and the survey should settle it rather than taste.
3. **Whether the mobile runner notifies on every sweep.** Silence is pleasant and
   also indistinguishable from the runner being dead, which is a bad property for
   a tool whose whole job is invisible.
