#!/usr/bin/env python3
"""Independently re-read the fixtures and check every declared claim.

Deliberately shares no parsing code with generate.py: if both agreed because
they agreed with each other, the manifest would prove nothing. This is also the
first sketch of the inspect phase.
"""
import json
import os
import struct
import subprocess
import sys
import zipfile
import zlib

OUT = sys.argv[1] if len(sys.argv) > 1 else "out"
ok = fail = 0


def check(name, cond, detail=""):
    global ok, fail
    if cond:
        ok += 1
        print(f"  \033[32mPASS\033[0m {name}" + (f"  {detail}" if detail else ""))
    else:
        fail += 1
        print(f"  \033[31mFAIL\033[0m {name}" + (f"  {detail}" if detail else ""))


def jpeg_segments(b):
    segs, i = [], 2
    while i < len(b) and b[i] == 0xFF:
        m = b[i + 1]
        if m == 0xDA:
            break
        ln = struct.unpack(">H", b[i + 2:i + 4])[0]
        segs.append((m, b[i + 4:i + 2 + ln]))
        i += 2 + ln
    return segs


def read_ifd(b, off, out):
    n = struct.unpack_from("<H", b, off)[0]
    for k in range(n):
        tag, tcode, cnt = struct.unpack_from("<HHI", b, off + 2 + 12 * k)
        raw = b[off + 10 + 12 * k:off + 14 + 12 * k]
        size = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 7: 1, 10: 8}[tcode] * cnt
        if size > 4:
            raw = b[struct.unpack("<I", raw)[0]:][:size]
        out[tag] = (tcode, cnt, raw)
    return struct.unpack_from("<I", b, off + 2 + 12 * n)[0]


def to_rat(raw):
    return [struct.unpack_from("<II", raw, 8 * i) for i in range(len(raw) // 8)]


print("\n== photo-phone.jpg ==")
b = open(f"{OUT}/photo-phone.jpg", "rb").read()
segs = jpeg_segments(b)
app1 = [p for m, p in segs if m == 0xE1]
exifseg = [p for p in app1 if p.startswith(b"Exif\0\0")][0][6:]
xmpseg = [p for p in app1 if p.startswith(b"http://ns.adobe.com/xap")][0]
ifd0, exif, gps, ifd1 = {}, {}, {}, {}
nxt = read_ifd(exifseg, struct.unpack_from("<I", exifseg, 4)[0], ifd0)
read_ifd(exifseg, struct.unpack("<I", ifd0[0x8769][2])[0], exif)
read_ifd(exifseg, struct.unpack("<I", ifd0[0x8825][2])[0], gps)
read_ifd(exifseg, nxt, ifd1)

check("Make/Model", ifd0[0x010F][2].startswith(b"Apple")
      and ifd0[0x0110][2].startswith(b"iPhone 14 Pro"))
check("Artist", ifd0[0x013B][2].startswith(b"Marie Dvorakova"))
check("body + lens serials", exif[0xA431][2].startswith(b"BX-4471902-TT")
      and exif[0xA435][2].startswith(b"LN-88213-04"))
latr, lonr = to_rat(gps[0x0002][2]), to_rat(gps[0x0004][2])
lat = latr[0][0] + latr[1][0] / 60 + (latr[2][0] / latr[2][1]) / 3600
lon = lonr[0][0] + lonr[1][0] / 60 + (lonr[2][0] / lonr[2][1]) / 3600
check("GPS decodes to Prague", abs(lat - 50.0755) < 0.001 and abs(lon - 14.4378) < 0.001,
      f"{lat:.4f}N {lon:.4f}E")
check("DateTimeOriginal", exif[0x9003][2].startswith(b"2024:06:14 19:42:07"))
check("XMP carries all three MM ids",
      all(k in xmpseg for k in (b"DocumentID", b"InstanceID", b"OriginalDocumentID")))
thoff = struct.unpack("<I", ifd1[0x0201][2])[0]
thlen = struct.unpack("<I", ifd1[0x0202][2])[0]
th = exifseg[thoff:thoff + thlen]
check("thumbnail extracts as a JPEG", th[:2] == b"\xff\xd8" and th[-2:] == b"\xff\xd9",
      f"{thlen} B")


def sof_size(j):
    for m, p in jpeg_segments(j):
        if 0xC0 <= m <= 0xC3:
            return struct.unpack_from(">HH", p, 1)[1], struct.unpack_from(">HH", p, 1)[0]
    return None


check("thumbnail differs in geometry from the main image",
      sof_size(th) == (160, 120) and sof_size(b) == (320, 240),
      f"thumb {sof_size(th)} vs main {sof_size(b)}")
check("ICC profile present (never-remove)",
      any(p.startswith(b"ICC_PROFILE\0") for m, p in segs if m == 0xE2))
iptc = [p for m, p in segs if m == 0xED]
check("IPTC By-line + City present",
      bool(iptc) and b"Marie Dvorakova" in iptc[0] and b"Praha" in iptc[0])

print("\n== screenshot.png ==")
b = open(f"{OUT}/screenshot.png", "rb").read()
chunks, i = {}, 8
while i < len(b):
    ln = struct.unpack_from(">I", b, i)[0]
    typ = b[i + 4:i + 8]
    chunks.setdefault(typ, []).append(b[i + 8:i + 8 + ln])
    i += 12 + ln
texts = b"\n".join(chunks.get(b"tEXt", []))
check("tEXt Author + Software + host comment",
      b"Marie Dvorakova" in texts and b"GNOME Screenshot" in texts and b"PRG-LT-0417" in texts)
check("tIME chunk decodes", chunks[b"tIME"][0] and
      struct.unpack(">HBBBBB", chunks[b"tIME"][0])[:3] == (2024, 6, 14))
check("eXIf chunk present and is TIFF", chunks[b"eXIf"][0][:2] == b"II")
check("iTXt carries XMP DocumentID", b"DocumentID" in chunks[b"iTXt"][0])
check("IDAT decompresses to 32 rows",
      len(zlib.decompress(chunks[b"IDAT"][0])) == 32 * 33)

print("\n== report-redacted.pdf ==")
b = open(f"{OUT}/report-redacted.pdf", "rb").read()
txt = b[b.find(b"4 0 obj"):b.find(b"endstream", b.find(b"4 0 obj"))]
check("redaction failure: covered text still extractable",
      b"CZ65 0800 0000 1920 0014 5399" in txt)
check("cover rect is painted AFTER the text",
      txt.find(b"re f") > txt.find(b"CZ65"))
check("trailer /ID pair present", b"/ID [<9f2c41a055b14e7c9b306d1e2a77c004>" in b)
check("/Author and /Producer in Info", b"/Author (Marie Dvorakova)" in b
      and b"Acrobat Distiller" in b)
check("OpenAction JavaScript present", b"/S /JavaScript" in b)
check("embedded file attachment present", b"/Type /EmbeddedFile" in b
      and b"settle below 400k" in b)
check("XMP DocumentID + InstanceID present",
      b"xmpMM:DocumentID" in b and b"xmpMM:InstanceID" in b)
# Payloads the catalog name tree does not mention, reachable only through the
# page: looking at the catalog alone finds neither.
check("a file is attached to the page through an annotation",
      b"/Subtype /FileAttachment" in b and b"payroll-draft.csv" in b
      and b"Jan Prochazka,184000" in b)
check("the annotation is on the page, not in the name tree",
      b"/Annots [12 0 R]" in b and b"/Names [(notes.txt)" in b
      and b"payroll" not in b[b.find(b"/Names [(notes.txt)"):b.find(b"/Names [(notes.txt)") + 120])
check("the page carries its own open action",
      b"/AA << /O 15 0 R >>" in b and b"mailDoc" in b)

print("\n== report-incremental.pdf ==")
b = open(f"{OUT}/report-incremental.pdf", "rb").read()
check("two startxref sections (incremental update)", b.count(b"startxref") == 2)
check("trailer chains via /Prev", b"/Prev " in b)
check("superseded v1 figure still recoverable", b"CZK 1,950,000" in b)
check("v2 replacement also present", b"[REDACTED]" in b)

print("\n== signed.pdf ==")
b = open(f"{OUT}/signed.pdf", "rb").read()
check("signature field detectable", b"/FT /Sig" in b and b"/SigFlags 3" in b)
check("/Type /Sig value object present", b"/Type /Sig" in b)

print("\n== letter.docx ==")
z = zipfile.ZipFile(f"{OUT}/letter.docx")
core = z.read("docProps/core.xml").decode()
app = z.read("docProps/app.xml").decode()
doc = z.read("word/document.xml").decode()
setx = z.read("word/settings.xml").decode()
com = z.read("word/comments.xml").decode()
check("dc:creator + cp:lastModifiedBy differ",
      "Marie Dvorakova" in core and "Petr Novak" in core)
check("cp:lastPrinted + revision present", "lastPrinted" in core and "<cp:revision>14" in core)
check("Company/Manager/Template/TotalTime present",
      all(k in app for k in ("Severin Holding", "Jan Prochazka",
                             "sev-internal-memo.dotx", "2471")))
check("w15:docId GUID present", "3F2A9C41-55B1-4E7C-9B30-6D1E2A77C004" in setx)
check("6 rsid save IDs present", setx.count("<w:rsid ") == 5 and "rsidRoot" in setx)
check("tracked insert + delete name their authors",
      "<w:ins " in doc and "<w:del " in doc and "CZK 400,000" in doc)
check("comment with author and initials", 'w:initials="PN"' in com
      and "legal signs off" in com)
th = z.read("docProps/thumbnail.jpeg")
check("embedded thumbnail is a real JPEG", th[:2] == b"\xff\xd8" and th[-2:] == b"\xff\xd9",
      f"{len(th)} B")
stamps = {i.date_time for i in z.infolist()}
check("every ZIP member carries one fixed stamp", stamps == {(2019, 3, 14, 9, 26, 52)},
      str(sorted(stamps)))
check("member order is Word-like, not alphabetical",
      [i.filename for i in z.infolist()][:3]
      == ["[Content_Types].xml", "_rels/.rels", "word/document.xml"])

print("\n== report.odt ==")
z = zipfile.ZipFile(f"{OUT}/report.odt")
infos = z.infolist()
# How a reader identifies an ODF package without unpacking it, and the reason
# rewriting this container is not the OOXML path with other part names.
check("mimetype is the first member", infos[0].filename == "mimetype", infos[0].filename)
check("mimetype is stored, not deflated",
      infos[0].compress_type == zipfile.ZIP_STORED, str(infos[0].compress_type))
check("mimetype declares an OpenDocument text",
      z.read("mimetype") == b"application/vnd.oasis.opendocument.text")
meta = z.read("meta.xml").decode()
check("meta.xml names both authors", "<meta:initial-creator>Marie Dvorakova" in meta
      and "<dc:creator>Petr Novak" in meta)
check("meta.xml records the generator", "LibreOffice/7.6.4.1" in meta)
check("meta.xml records the editing history",
      "<meta:editing-cycles>14" in meta and "PT41H11M" in meta)
check("meta.xml carries a matter number", "SEV-2024-0188" in meta)
check("settings.xml names the printer", "HP-LaserJet-M404-PRAHA-4F" in
      z.read("settings.xml").decode())
content = z.read("content.xml").decode()
check("content.xml keeps the deleted figure and its author",
      "CZK 1,950,000" in content and "<dc:creator>Petr Novak" in content)
check("content.xml carries an annotation naming someone", "Marie Dvorakova" in content)
check("a first-page preview is in the package",
      "Thumbnails/thumbnail.png" in z.namelist())
# None of these part names exist in OOXML: read with the OOXML vocabulary the
# package has no properties at all, which is why the engine must refuse it.
check("no part name overlaps the OOXML set",
      not any(n.startswith("docProps/") or n == "[Content_Types].xml" for n in z.namelist()))

print("\n== diagram.svg ==")
s = open(f"{OUT}/diagram.svg").read()
check("inkscape version + sodipodi docname present",
      "inkscape:version" in s and "org-chart-INTERNAL-v4.svg" in s)
check("dc:creator present", "Marie Dvorakova" in s)
check("off-canvas text outside the viewBox", 'y="400"' in s
      and 'viewBox="0 0 200 100"' in s and "-18 FTE Q3" in s)
# The two traps: a bleed rect is visible artwork anchored off the canvas, and
# the parked text comes after a transform group that has already closed.
check("a bleed rect is anchored off the canvas and covers it",
      'x="-10" y="-10" width="220" height="120"' in s)
check("the parked text follows a closed transform group",
      s.index("</g>") < s.index('y="400"') and s.count("<g transform") == 1)
check("the transform group holds visible artwork",
      s.index('x="10" y="20"') > s.index("<g transform"))

print("\n== clip.mp4 ==")
b = open(f"{OUT}/clip.mp4", "rb").read()


def udta_text(typ):
    i = b.find(typ)
    if i < 0:
        return None
    n, _lang = struct.unpack_from(">HH", b, i + 4)
    return b[i + 8:i + 8 + n].decode("utf-8", "replace")


check("udta GPS atom decodes", udta_text(b"\xa9xyz") == "+50.0755+014.4378/",
      str(udta_text(b"\xa9xyz")))
check("udta make/model decode", udta_text(b"\xa9mak") == "Apple"
      and udta_text(b"\xa9mod") == "iPhone 14 Pro")
check("udta date atom decodes", udta_text(b"\xa9dat") == "2024-06-14T19:42:07Z")
check("3GPP loci box also present", b"loci" in b)
check("iTunes ilst artist + comment present",
      b"Marie Dvorakova" in b and b"do not publish" in b)
m = b.find(b"mvhd")
check("mvhd creation time is the declared instant (1904 epoch)",
      struct.unpack_from(">I", b, m + 8)[0] == 1718394127 + 2082844800,
      str(struct.unpack_from(">I", b, m + 8)[0]))
# Atom surgery grew moov; if any size field were wrong the file would not demux.
r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration",
                    "-of", "default=nw=1:nk=1", f"{OUT}/clip.mp4"],
                   capture_output=True, text=True)
check("still demuxes after udta injection", r.returncode == 0
      and r.stdout.strip().startswith("1.0"), r.stdout.strip() or r.stderr.strip()[:60])

print("\n== clip-ios.mov ==")
b = open(f"{OUT}/clip-ios.mov", "rb").read()
# The keys dictionary and a numbered ilst: the values carry no atom type, so
# the names live only in `keys` and nothing but an index joins the two.
ki = b.find(b"keys")
check("keys dictionary present", ki > 0)
n_keys = struct.unpack_from(">I", b, ki + 8)[0] if ki > 0 else 0
check("keys declares its entry count", 1 <= n_keys <= 32, str(n_keys))
names = []
if ki > 0:
    at = ki + 12
    for _ in range(n_keys):
        sz = struct.unpack_from(">I", b, at)[0]
        if sz < 8:
            break
        names.append(b[at + 8:at + sz].decode("utf-8", "replace"))
        at += sz
check("the dictionary names the device keys",
      "com.apple.quicktime.make" in names and "com.apple.quicktime.model" in names,
      ", ".join(names))
check("the dictionary names the location key",
      "com.apple.quicktime.location.ISO6709" in names)
check("the dictionary names the per-device signature",
      "com.apple.photos.originating.signature" in names)
# The first child of `ilst` begins four bytes past the type: its own type is
# a key index rather than four printable characters.
li = b.find(b"ilst", ki)
check("ilst entries are numbered, not named",
      li > 0 and struct.unpack_from(">I", b, li + 8)[0] in range(1, n_keys + 1),
      str(struct.unpack_from(">I", b, li + 8)[0]) if li > 0 else "no ilst")
check("the values are in the file", b"+50.0755+014.4378/" in b
      and b"iPhone 14 Pro" in b
      and b"9F2C14D8-7A5B-4E10-8C33-0B6E1D5A77C4" in b)
r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format_tags",
                    "-of", "default=nw=1", f"{OUT}/clip-ios.mov"],
                   capture_output=True, text=True)
check("ffprobe reads the apple keys back", r.returncode == 0
      and "com.apple.quicktime.model=iPhone 14 Pro" in r.stdout,
      r.stdout.strip().replace("\n", "; ")[:90] or r.stderr.strip()[:60])

print("\n== clip-truncated.mp4 ==")
t = open(f"{OUT}/clip-truncated.mp4", "rb").read()
check("shorter than the clip it came from", len(t) == len(open(f"{OUT}/clip.mp4", "rb").read()) - 15,
      f"{len(t)} bytes")
# The declared point of this fixture: the fields are still there, and the box
# chain does not reach them.
check("the declared fields are still in the bytes", b"iPhone 14 Pro" in t
      and b"+50.0755+014.4378/" in t and b"Marie Dvorakova" in t)
at, covered = 0, True
while at + 8 <= len(t):
    sz = struct.unpack_from(">I", t, at)[0]
    if sz < 8 or at + sz > len(t):
        covered = False
        break
    at += sz
check("the top-level box chain does not cover the file", not covered or at != len(t),
      f"stops at {at} of {len(t)}")
# And this is why a field count on this file is a lie rather than a shortfall:
# a tolerant reader still hands out every one of the declared values.
r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format_tags",
                    "-of", "default=nw=1", f"{OUT}/clip-truncated.mp4"],
                   capture_output=True, text=True)
check("an independent reader still sees the declared fields",
      "iPhone 14 Pro" in r.stdout and "+50.0755+014.4378/" in r.stdout
      and "Marie Dvorakova" in r.stdout,
      r.stdout.strip().replace("\n", "; ")[:80] or r.stderr.strip()[:60])

print("\n== filesystem ==")
st = os.stat(f"{OUT}/photo-phone.jpg")
check("mtime pinned to the declared instant", int(st.st_mtime) == 1718394127,
      f"{int(st.st_mtime)}")
try:
    check("user.figmun.test xattr readable",
          os.getxattr(f"{OUT}/photo-phone.jpg", b"user.figmun.test") == b"declared-fixture")
except OSError as e:
    check("user.figmun.test xattr readable", False, str(e))
check("Zone.Identifier stand-in sidecar present",
      "ZoneId=3" in open(f"{OUT}/photo-phone.jpg.Identifier").read())

print("\n== manifest ==")
man = json.load(open("manifest.json"))
declared = {k for k in man["fixtures"] if not k.startswith("_")}
present = {f for f in os.listdir(OUT) if not f.endswith(".Identifier")}
check("every declared fixture exists on disk", declared == present,
      str(declared ^ present) if declared != present else f"{len(declared)} files")

print(f"\n  {ok} passed, {fail} failed\n")
sys.exit(1 if fail else 0)
