# Figura Munda

Shows you what a file reveals about you, then removes it.

*Munda* as in clean. The adjective agrees with *figura*, following Figura Obscura.

> **Status:** planning only. Nothing built yet. This is the pick-up-from-here document.

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
   test suite, for free.
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

`uniform` is the one that makes two files from different people indistinguishable,
and it is also the one most likely to look odd to a recipient. `custom` overrides any
cell. Profile choice is the single decision a user makes.

### Never remove

Stripping these is a regression, not a protection: ICC colour profiles (the image
changes appearance), tagged-PDF accessibility structure, and anything inside a
**signed** PDF. Detect signatures and refuse rather than silently invalidating them.

## Coverage

JPEG, PNG, HEIC, TIFF · PDF · DOCX, XLSX, PPTX, ODF · MP4, MOV · SVG

## Trust posture

No network, ever. No accounts, no telemetry, no model downloads. Input and output
both sit on disk and can be diffed by anyone who doubts the claim. Reproducible
builds so a published binary can be checked against this source.

## Deliverables

- **`desktop/`** Rust, plus shell integration (Explorer, Nautilus, Dolphin, macOS
  Quick Actions). Packaging follows the path already proven by Figura Obscura.
- **`mobile/`** the share sheet, which is the natural intercept and arguably the more
  important target, since geotagged photographs originate on the phone. iOS Share
  Extension, Android `ACTION_SEND`.

**No browser extension.** Transparent upload interception is not buildable: MV3's
`declarativeNetRequest` cannot touch request bodies, blocking `webRequest` is gone,
and `requestBody` is read-only even in Firefox. The MAIN-world JS-patching workaround
breaks on every site redesign, which is a maintenance tax this project exists to
avoid.

## Open decisions

1. **Licence.** Open core with a private paid layer, or PolyForm Noncommercial over
   everything. Must be settled before the first public commit, along with
   contribution terms (a CLA, or a paid layer in a separate repo sharing no code).
   The closed half must never be security-relevant: scheduling and reporting may be
   closed, the cleaning engine never.
2. **Leakage survey.** Which destinations actually leak? Major social platforms
   re-encode and strip EXIF anyway, so the real exposure is webmail attachments,
   forums, file hosts and application portals. This determines whether mobile or
   desktop ships first.
3. **Mobile or desktop first**, pending that survey.
