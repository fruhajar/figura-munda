# Fixtures

Generated test files only. **Never commit a real document.**

A test corpus for a metadata tool is, by construction, a pile of other people's
personal data. Every fixture here is written byte by byte by `generate.py`, so the
metadata it carries is known exactly rather than inherited from an imaging library's
defaults. Real-world samples belong in `/corpus`, which is gitignored and stays local.

```
python3 generate.py      # writes out/ and manifest.json
python3 verify.py        # re-reads out/ and checks every declared claim
python3 checkpdf.py F... # structural check of any PDF, including cleaned output
```

`verify.py` checks the inputs. `checkpdf.py` checks outputs, and exists because
reading a cleaned PDF back through the library that wrote it proves that library
is self-consistent and nothing else. It parses the cross-reference table by hand
and asserts every offset lands on the object it claims to, which is the failure
a self-consistent library hides and which produces a file every other reader
rejects. The Rust test suite runs it over every cleaned fixture.

`out/` is gitignored. Regenerate rather than edit, and never hand-edit
`manifest.json`.

## The set

| Fixture | Classes exercised |
|---|---|
| `photo-phone.jpg` | identity, device, location, time, correlation, hidden payload, never-remove |
| `photo-codepage.jpg` | identity, device, written in Windows-1252 with nothing in the file declaring it |
| `photo-oriented.png` | device, time, and **never-remove**: an orientation and a colour profile inside a block that is itself worth dropping |
| `screenshot.png` | identity, device, time, correlation |
| `report-redacted.pdf` | identity, device, time, correlation, **redaction integrity**, hidden payload |
| `report-incremental.pdf` | history (prior revision fully recoverable) |
| `signed.pdf` | never-remove (detect the signature and refuse) |
| `encrypted.pdf` | never-remove (detect `/Encrypt` and refuse; cleaning destroys such a file) |
| `nested.pdf` | hostile, not broken: 20000 matched nested arrays, structurally sound, refused by depth before parsing |
| `letter.docx` | identity, time, correlation, history, hidden payload, structural fingerprint |
| `diagram.svg` | identity, device, time, correlation, hidden payload |
| `report.odt` | identity, device, time, history, hidden payload, none of it read yet: the declared contract is a refusal |
| `clip.mp4` | identity, device, location, time |
| `clip-ios.mov` | device, location, time, correlation, all of it named only by a `keys` dictionary |
| `clip-truncated.mp4` | an interrupted copy: every field still in the bytes, unreachable, and `ffprobe` still hands them out |
| (all of them) | filesystem: pinned mtime, xattr, Zone.Identifier stand-in |

That covers all ten classes in the root README.

## Cases worth knowing about

**The JPEG thumbnail is deliberately a different picture.** The main image is a
320x240 flat colour; the EXIF IFD1 thumbnail is a 160x120 test pattern. They cannot
be mistaken for scaled copies of each other, which is what makes the "uncropped
original survives inside the crop" leak visible at a glance in a report.

**The redacted PDF draws the text first and the black rectangle second.** The verifier
asserts both that the account number is still extractable and that the covering
rectangle appears later in the content stream than the text it hides.

**The incremental PDF keeps the real settlement figure.** v2 overwrites objects 4 and
6 and the trailer chains back via `/Prev`, so the superseded v1 is recoverable in
full. Two `startxref` sections are the tell.

**`signed.pdf` is structurally a signature, not a cryptographic one.** `/Contents` is a
stub. It exists so the detect-and-refuse path has a target; do not use it to test
signature validation.

**The ICC profile is a 0-tag stub.** Structurally valid, not a usable colour
transform. It is there to test that the never-remove rule holds, not colour accuracy.

## Two findings the verifier produced

Both were declaration bugs caught because `verify.py` deliberately shares no parsing
code with `generate.py`. If both sides agreed only with each other, the manifest would
prove nothing.

1. **ZIP timestamps have 2-second granularity.** MS-DOS time stores seconds halved, so
   an odd second is unrepresentable. `09:26:53` silently became `09:26:52` on the
   round trip. Any assertion on OOXML member timestamps must use even seconds.
2. **ffmpeg does not write the atoms a phone writes.** It emits a 3GPP `loci` box and
   an iTunes-style `ilst`, never `©xyz`. Real handset video carries `moov/udta/©xyz`,
   so the generator injects `©xyz`, `©mak`, `©mod` and `©dat` by hand and grows `udta`
   and `moov` to match. That is only safe because ffmpeg places `moov` after `mdat`
   here; growing an atom ahead of the media would invalidate every `stco` offset. The
   verifier runs `ffprobe` afterwards to confirm the file still demuxes.
