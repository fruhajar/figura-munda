#!/usr/bin/env python3
"""Generate the Figura Munda fixture set.

Every value written here is declared in DECLARED below, so the inspect phase has
exact ground truth to assert against. Nothing is inherited from an imaging
library, which is the point: a fixture whose metadata came from Pillow or ffmpeg
is only as known as those tools' defaults.

Run: python3 generate.py [outdir]
"""
import binascii
import json
import os
import struct
import subprocess
import sys
import zipfile
import zlib

import exifbuild as E

OUT = sys.argv[1] if len(sys.argv) > 1 else "out"

# A stable per-device handle, the kind that correlates files without naming anyone.
MOV_SIGNATURE = "9F2C14D8-7A5B-4E10-8C33-0B6E1D5A77C4"

# A real phone photo's worth of plausible values. Prague, so the GPS class has a
# recognisable target when someone eyeballs a report.
LAT = (50, 4, 31.8)
LON = (14, 26, 16.1)
SERIAL_BODY = "BX-4471902-TT"
SERIAL_LENS = "LN-88213-04"
AUTHOR = "Marie Dvorakova"
# Accented, and spelled the same in Windows-1252 and Latin-1, so what the
# report shows is checkable rather than a guess about codepages.
CP_AUTHOR = "Jürgen Müller"
DOC_ID = "xmp.did:9F2C41A0-55B1-4E7C-9B30-6D1E2A77C004"
INST_ID = "xmp.iid:1B4E7761-2C09-4F55-A0D8-3E9C5577B112"
ORIG_ID = "xmp.oid:7C55B1A0-9F2C-41E7-B930-004C6D1E2A77"
WORD_DOCID = "{3F2A9C41-55B1-4E7C-9B30-6D1E2A77C004}"
PDF_FILE_ID = "9f2c41a055b14e7c9b306d1e2a77c004"
TS = "2024:06:14 19:42:07"


def rat(deg):
    d, m, s = deg
    return E.rational_((d, 1), (m, 1), (int(s * 100), 100))


# --------------------------------------------------------------------------- #
# JPEG
# --------------------------------------------------------------------------- #

def icc_stub():
    """Structurally valid 0-tag ICC profile. Present so the never-remove rule
    has a target; not a usable colour transform."""
    body = struct.pack(">I", 132) + b"ADBE" + struct.pack(">I", 0x02100000)
    body += b"mntrRGB XYZ "
    body += b"\0" * (128 - len(body))
    return body + struct.pack(">I", 0)


def iptc_block():
    """Photoshop IRB carrying an 8BIM 0x0404 IPTC-IIM record."""
    def ds(record, dataset, value):
        v = value.encode("latin-1")
        return struct.pack(">BBBH", 0x1C, record, dataset, len(v)) + v

    iptc = ds(2, 80, AUTHOR) + ds(2, 90, "Praha") + ds(2, 101, "Czech Republic")
    iptc += ds(2, 120, "Internal only - do not circulate")
    if len(iptc) % 2:
        iptc += b"\0"
    return b"Photoshop 3.0\0" + b"8BIM" + struct.pack(">H", 0x0404) + b"\0\0" + \
           struct.pack(">I", len(iptc)) + iptc


XMP_IMG = f"""<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmp:CreatorTool="Lightroom Classic 13.2"
    xmp:MetadataDate="2024-06-14T21:03:11+02:00"
    xmpMM:DocumentID="{DOC_ID}"
    xmpMM:InstanceID="{INST_ID}"
    xmpMM:OriginalDocumentID="{ORIG_ID}">
   <dc:creator><rdf:Seq><rdf:li>{AUTHOR}</rdf:li></rdf:Seq></dc:creator>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"""


def jpeg_tail(data):
    """Return everything from the first non-APP/COM segment onward."""
    i = 2
    while i < len(data):
        assert data[i] == 0xFF, i
        m = data[i + 1]
        if 0xE0 <= m <= 0xEF or m == 0xFE:  # APPn, COM
            i += 2 + struct.unpack(">H", data[i + 2:i + 4])[0]
            continue
        return data[i:]
    raise AssertionError("no image segments")


def seg(marker, payload):
    return struct.pack(">BBH", 0xFF, marker, len(payload) + 2) + payload


def make_jpeg(path):
    base = subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi",
         "-i", "color=c=0x6b7a8f:s=320x240", "-frames:v", "1",
         "-f", "image2", "-c:v", "mjpeg", "-"],
        capture_output=True, check=True).stdout
    # Deliberately wider and visually different: this is the uncropped original
    # surviving inside the "cropped" file.
    thumb = subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi",
         "-i", "testsrc=s=160x120", "-frames:v", "1",
         "-f", "image2", "-c:v", "mjpeg", "-"],
        capture_output=True, check=True).stdout

    ifd0 = {
        0x010F: E.ascii_("Apple"),
        0x0110: E.ascii_("iPhone 14 Pro"),
        0x0131: E.ascii_("17.4.1"),
        0x0112: E.short_(6),
        0x013B: E.ascii_(AUTHOR),
        0x8298: E.ascii_(f"(c) 2024 {AUTHOR}"),
        0x010E: E.ascii_("Site visit, do not publish"),
        0x0132: E.ascii_(TS),
    }
    exif = {
        0x9000: E.undef_(b"0232"),
        0x9003: E.ascii_(TS),
        0x9004: E.ascii_(TS),
        0xA431: E.ascii_(SERIAL_BODY),
        0xA435: E.ascii_(SERIAL_LENS),
        0xA434: E.ascii_("iPhone 14 Pro back triple camera 6.86mm f/1.78"),
        0x927C: E.undef_(b"AppleMakerNote\0" + b"\x01\x02" + SERIAL_BODY.encode()),
        0xA420: E.ascii_("B7A41E2C55B14E7C9B306D1E2A77C004"),
    }
    gps = {
        0x0000: E.byte_(b"\x02\x03\x00\x00"),
        0x0001: E.ascii_("N"),
        0x0002: rat(LAT),
        0x0003: E.ascii_("E"),
        0x0004: rat(LON),
        0x0005: E.byte_(b"\x00"),
        0x0006: E.rational_((21740, 100)),
        0x0007: E.rational_((17, 1), (42, 1), (7, 1)),
        0x001D: E.ascii_("2024:06:14"),
    }
    ifd1 = {0x0103: E.short_(6), 0x011A: E.rational_((72, 1)),
            0x011B: E.rational_((72, 1)), 0x0128: E.short_(2)}

    out = b"\xff\xd8"
    out += seg(0xE1, b"Exif\0\0" + E.build_exif(ifd0, exif, gps, ifd1, thumb))
    out += seg(0xE1, b"http://ns.adobe.com/xap/1.0/\0" + XMP_IMG.encode("utf-8"))
    out += seg(0xE2, b"ICC_PROFILE\0\x01\x01" + icc_stub())
    out += seg(0xED, iptc_block())
    out += jpeg_tail(base)
    open(path, "wb").write(out)
    return {"thumb_bytes": len(thumb), "total_bytes": len(out), "_thumb": thumb}


def make_jpeg_codepage(path):
    """EXIF text in Windows-1252 rather than UTF-8, which is what a Windows
    tool wrote for most of the last thirty years.

    EXIF says ASCII and these fields are not, so there is nothing in the file
    that declares the encoding. Read as UTF-8 every accented letter becomes a
    replacement character, and a report full of those says less about the file
    than the file does.
    """
    base = subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi",
         "-i", "color=c=0x8f7a6b:s=64x48", "-frames:v", "1",
         "-f", "image2", "-c:v", "mjpeg", "-"],
        capture_output=True, check=True).stdout

    def cp(text):
        return E.ascii_(text.encode("cp1252").decode("latin-1"))

    ifd0 = {
        0x010F: cp("Müller Zeichenbüro"),
        0x013B: cp(CP_AUTHOR),
        0x8298: cp(f"(c) 2024 {CP_AUTHOR}"),
        0x010E: cp("Lageplan für Köln-Süd, nicht veröffentlichen"),
    }
    out = b"\xff\xd8"
    out += seg(0xE1, b"Exif\0\0" + E.build_exif(ifd0, {}, {}, {}, b""))
    out += jpeg_tail(base)
    open(path, "wb").write(out)
    assert CP_AUTHOR.encode("cp1252") in out, "the fixture is not in a codepage at all"
    assert CP_AUTHOR.encode("utf-8") not in out, "the fixture is UTF-8 after all"
    return {"total_bytes": len(out), "author_bytes": CP_AUTHOR.encode("cp1252").hex()}


# --------------------------------------------------------------------------- #
# PNG
# --------------------------------------------------------------------------- #

def chunk(typ, data):
    return (struct.pack(">I", len(data)) + typ + data
            + struct.pack(">I", zlib.crc32(typ + data) & 0xFFFFFFFF))


def make_png(path):
    w = h = 32
    raw = b"".join(b"\0" + bytes((x * 8 + y) % 256 for x in range(w)) for y in range(h))
    exif = E.build_exif(
        {0x0131: E.ascii_("GNOME Screenshot 45.1"), 0x0132: E.ascii_(TS)},
        {0x9003: E.ascii_(TS)}, {0x0000: E.byte_(b"\x02\x03\x00\x00")},
        {0x0103: E.short_(6)}, b"\xff\xd8\xff\xd9")
    xmp = (b"XML:com.adobe.xmp\0\0\0\0\0"
           + XMP_IMG.replace("Lightroom Classic 13.2", "GNOME Screenshot 45.1").encode())
    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 0, 0, 0, 0))
    out += chunk(b"tEXt", b"Author\0" + AUTHOR.encode("latin-1"))
    out += chunk(b"tEXt", b"Software\0GNOME Screenshot 45.1")
    out += chunk(b"tEXt", b"Comment\0Captured on workstation PRG-LT-0417")
    out += chunk(b"iTXt", xmp)
    out += chunk(b"eXIf", exif)
    out += chunk(b"tIME", struct.pack(">HBBBBB", 2024, 6, 14, 17, 42, 7))
    out += chunk(b"pHYs", struct.pack(">IIB", 2835, 2835, 1))
    out += chunk(b"IDAT", zlib.compress(raw, 9))
    out += chunk(b"IEND", b"")
    open(path, "wb").write(out)
    return {"total_bytes": len(out)}


# --------------------------------------------------------------------------- #
# PDF
# --------------------------------------------------------------------------- #

def assemble_pdf(objs, trailer_extra, root_num, start_at=0, prev=None, prefix=b""):
    """objs: {num: bytes}. Returns full file bytes with an xref table."""
    out = bytearray(prefix)
    if not prefix:
        out += b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n"
    offsets = {}
    for n in sorted(objs):
        offsets[n] = len(out)
        out += b"%d 0 obj\n" % n + objs[n] + b"\nendobj\n"
    xref_at = len(out)
    hi = max(objs) + 1
    if prev is None:
        out += b"xref\n0 %d\n" % hi
        out += b"0000000000 65535 f \n"
        for n in range(1, hi):
            out += (b"%010d 00000 n \n" % offsets[n]) if n in offsets else b"0000000000 65535 f \n"
    else:
        out += b"xref\n"
        for n in sorted(objs):
            out += b"%d 1\n%010d 00000 n \n" % (n, offsets[n])
    out += b"trailer\n<< /Size %d /Root %d 0 R %s" % (hi, root_num, trailer_extra)
    if prev is not None:
        out += b" /Prev %d" % prev
    out += b" >>\nstartxref\n%d\n%%%%EOF\n" % xref_at
    return bytes(out), xref_at


XMP_PDF = f"""<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
 xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about=""
 xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
 xmlns:pdf="http://ns.adobe.com/pdf/1.3/" xmlns:dc="http://purl.org/dc/elements/1.1/"
 xmpMM:DocumentID="{DOC_ID}" xmpMM:InstanceID="{INST_ID}"
 xmp:CreatorTool="Microsoft Word 2021" pdf:Producer="Acrobat Distiller 23.0">
 <dc:creator><rdf:Seq><rdf:li>{AUTHOR}</rdf:li></rdf:Seq></dc:creator>
</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"""

SECRET = "Claimant bank account CZ65 0800 0000 1920 0014 5399"
INFO = (b"<< /Title (Quarterly review - FINAL) /Author (" + AUTHOR.encode()
        + b") /Subject (Internal) /Keywords (confidential, draft) "
        b"/Creator (Microsoft Word 2021) /Producer (Acrobat Distiller 23.0) "
        b"/CreationDate (D:20240614194207+02'00') "
        b"/ModDate (D:20240615081133+02'00') >>")


def make_pdf_redacted(path):
    # Text is drawn, then a filled rectangle is painted over it. Visually
    # redacted, still fully extractable.
    content = (b"BT /F1 11 Tf 72 720 Td (Quarterly review) Tj ET\n"
               b"BT /F1 11 Tf 72 700 Td (" + SECRET.encode() + b") Tj ET\n"
               b"0 0 0 rg 68 694 420 20 re f\n")
    objs = {
        1: b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles 9 0 R >> "
           b"/OpenAction 8 0 R /Metadata 7 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        # The payloads the catalog name tree does not mention: a file attached
        # to the page through an annotation, and the page's own open action.
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] "
           b"/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R "
           b"/Annots [12 0 R] /AA << /O 15 0 R >> >>",
        4: b"<< /Length %d >>\nstream\n" % len(content) + content + b"endstream",
        5: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        7: b"<< /Type /Metadata /Subtype /XML /Length %d >>\nstream\n"
           % len(XMP_PDF.encode()) + XMP_PDF.encode() + b"\nendstream",
        8: b"<< /S /JavaScript /JS (app.alert\\('opened by ' + identity.name\\);) >>",
        9: b"<< /Names [(notes.txt) 10 0 R] >>",
        10: b"<< /Type /Filespec /F (notes.txt) /EF << /F 11 0 R >> >>",
        11: b"<< /Type /EmbeddedFile /Length 48 >>\nstream\n"
            b"internal: settle below 400k, do not disclose\n\nendstream",
        12: b"<< /Type /Annot /Subtype /FileAttachment /Rect [500 800 520 820] "
            b"/Contents (draft) /FS 13 0 R >>",
        13: b"<< /Type /Filespec /F (payroll-draft.csv) /EF << /F 14 0 R >> >>",
        14: b"<< /Type /EmbeddedFile /Length 31 >>\nstream\n"
            b"name,gross\nJan Prochazka,184000\n\nendstream",
        15: b"<< /S /JavaScript /JS (this.mailDoc\\({cTo: 'audit@severin.example'}\\);) >>",
        6: INFO,
    }
    ident = (b"/ID [<" + PDF_FILE_ID.encode() + b"> <" + PDF_FILE_ID.encode() + b">]"
             b" /Info 6 0 R")
    data, _ = assemble_pdf(objs, ident, 1)
    open(path, "wb").write(data)
    return {"total_bytes": len(data), "hidden_text": SECRET}


def make_pdf_incremental(path):
    """v1 says the real figure; v2 overwrites the page. v1 stays recoverable."""
    v1 = (b"BT /F1 11 Tf 72 720 Td (Settlement offer: CZK 1,950,000) Tj ET\n")
    objs = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] "
           b"/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
        4: b"<< /Length %d >>\nstream\n" % len(v1) + v1 + b"endstream",
        5: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        6: b"<< /Title (Offer v1) /Author (" + AUTHOR.encode() + b") >>",
    }
    base, xref1 = assemble_pdf(objs, b"/Info 6 0 R", 1)
    v2 = b"BT /F1 11 Tf 72 720 Td (Settlement offer: [REDACTED]) Tj ET\n"
    upd = {4: b"<< /Length %d >>\nstream\n" % len(v2) + v2 + b"endstream",
           6: b"<< /Title (Offer v2 - cleared for release) >>"}
    data, _ = assemble_pdf(upd, b"/Info 6 0 R", 1, prev=xref1, prefix=base)
    open(path, "wb").write(data)
    return {"total_bytes": len(data), "recoverable_v1": "CZK 1,950,000"}


def make_pdf_signed(path):
    """Structurally a signature field, not a real cryptographic signature. Enough
    for the detect-and-refuse path to have a target."""
    objs = {
        1: b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] "
           b"/SigFlags 3 >> >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] "
           b"/Annots [6 0 R] /Contents 4 0 R >>",
        4: b"<< /Length 46 >>\nstream\nBT /F1 11 Tf 72 720 Td (Signed mandate) Tj ET\nendstream",
        6: b"<< /FT /Sig /T (Signature1) /Type /Annot /Subtype /Widget "
           b"/Rect [72 600 300 660] /V 7 0 R >>",
        7: b"<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /ETSI.CAdES.detached "
           b"/M (D:20240614194207+02'00') /Name (" + AUTHOR.encode()
           + b") /Contents <308006092a864886f70d010702a080> >>",
        8: b"<< /Author (" + AUTHOR.encode() + b") /Producer (Acrobat Distiller 23.0) >>",
    }
    data, _ = assemble_pdf(objs, b"/Info 8 0 R", 1)
    open(path, "wb").write(data)
    return {"total_bytes": len(data)}


# --------------------------------------------------------------------------- #
# DOCX
# --------------------------------------------------------------------------- #

CORE = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
 xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/"
 xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
 <dc:title>Quarterly review</dc:title><dc:subject>Internal</dc:subject>
 <dc:creator>{AUTHOR}</dc:creator>
 <cp:keywords>confidential, draft</cp:keywords>
 <cp:lastModifiedBy>Petr Novak</cp:lastModifiedBy>
 <cp:revision>14</cp:revision>
 <cp:lastPrinted>2024-06-13T16:08:00Z</cp:lastPrinted>
 <dcterms:created xsi:type="dcterms:W3CDTF">2024-05-02T08:14:00Z</dcterms:created>
 <dcterms:modified xsi:type="dcterms:W3CDTF">2024-06-15T06:11:00Z</dcterms:modified>
</cp:coreProperties>"""

APP = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
 <Application>Microsoft Office Word</Application><AppVersion>16.0000</AppVersion>
 <Company>Severin Holding a.s.</Company><Manager>Jan Prochazka</Manager>
 <Template>sev-internal-memo.dotx</Template><TotalTime>2471</TotalTime>
 <Pages>3</Pages><Words>812</Words>
</Properties>"""

DOCUMENT = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
 <w:body>
  <w:p w:rsidR="00A41C2F" w:rsidRDefault="00B7E309">
   <w:r><w:t>Quarterly review</w:t></w:r></w:p>
  <w:p w:rsidR="00C55B1A">
   <w:ins w:id="7" w:author="Marie Dvorakova" w:date="2024-06-14T19:42:00Z">
    <w:r><w:t>Settlement authority raised to CZK 1,950,000.</w:t></w:r></w:ins>
   <w:del w:id="8" w:author="Petr Novak" w:date="2024-06-15T06:10:00Z">
    <w:r><w:delText>Previous cap was CZK 400,000.</w:delText></w:r></w:del>
  </w:p>
  <w:p><w:commentRangeStart w:id="1"/><w:r><w:t>Figures to confirm.</w:t></w:r>
   <w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>
 </w:body>
</w:document>"""

SETTINGS = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
 xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml">
 <w15:docId w15:val="{WORD_DOCID}"/>
 <w:rsids><w:rsidRoot w:val="00B7E309"/>
  <w:rsid w:val="00A41C2F"/><w:rsid w:val="00B7E309"/><w:rsid w:val="00C55B1A"/>
  <w:rsid w:val="006D1E2A"/><w:rsid w:val="0077C004"/></w:rsids>
</w:settings>"""

COMMENTS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
 <w:comment w:id="1" w:author="Petr Novak" w:initials="PN" w:date="2024-06-15T06:09:00Z">
  <w:p><w:r><w:t>Do not send to the claimant before legal signs off.</w:t></w:r></w:p>
 </w:comment>
</w:comments>"""

CT = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="jpeg" ContentType="image/jpeg"/>
 <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
 <Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/>
 <Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/>
 <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
 <Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
</Types>"""

RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
 <Relationship Id="rId4" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail" Target="docProps/thumbnail.jpeg"/>
</Relationships>"""

DRELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>
 <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/>
</Relationships>"""

# Non-alphabetical, Word-like member order; the fixed timestamp is itself a
# structural fingerprint and must survive into the manifest.
ZIP_TS = (2019, 3, 14, 9, 26, 52)


def make_pdf_nested(path, depth=20000):
    """Arrays nested deep enough to overflow a recursive parser's stack.

    Not malformed: every bracket is matched and the cross-reference table is
    correct. That is the point. A parser that descends recursively dies on it,
    and a stack overflow is not a panic, so the process aborts with a core dump
    and takes the whole folder run with it. The engine has to decide this one
    before parsing it, which means counting depth lexically.
    """
    nested = b"[" * depth + b"]" * depth
    objs = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>",
        4: b"<< /Length 0 >>\nstream\n\nendstream",
        5: b"<< /Deep " + nested + b" >>",
    }
    data, _ = assemble_pdf(objs, b"", 1)
    open(path, "wb").write(data)
    return {"total_bytes": len(data), "nesting_depth": depth}


def make_pdf_encrypted(path):
    """A PDF declaring the standard security handler.

    The /O and /U strings are stubs rather than real key material, as with
    signed.pdf: what matters is that the file says it is encrypted. Cleaning it
    cannot work, because the strings and streams of a real one are ciphertext
    and the trailer /ID is an input to the key, so a rewrite leaves a file no
    reader will open.
    """
    body = b"BT /F1 11 Tf 72 720 Td (Ciphertext in a real one.) Tj ET\n"
    objs = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>",
        4: b"<< /Length %d >>\nstream\n" % len(body) + body + b"endstream",
        5: (b"<< /Filter /Standard /V 2 /R 3 /Length 128 /P -3904 "
            b"/O <2d3b4c5a6e7f8091a2b3c4d5e6f70812> "
            b"/U <9f2c41a055b14e7c9b306d1e2a77c004> >>"),
        6: INFO,
    }
    ident = (b"/ID [<" + PDF_FILE_ID.encode() + b"> <" + PDF_FILE_ID.encode() + b">]"
             b" /Info 6 0 R /Encrypt 5 0 R")
    data, _ = assemble_pdf(objs, ident, 1)
    open(path, "wb").write(data)
    return {"total_bytes": len(data), "encrypt_object": 5}


def make_docx(path, thumb):
    members = [
        ("[Content_Types].xml", CT), ("_rels/.rels", RELS),
        ("word/document.xml", DOCUMENT), ("word/_rels/document.xml.rels", DRELS),
        ("word/settings.xml", SETTINGS), ("word/comments.xml", COMMENTS),
        ("docProps/core.xml", CORE), ("docProps/app.xml", APP),
    ]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in members:
            zi = zipfile.ZipInfo(name, date_time=ZIP_TS)
            zi.compress_type = zipfile.ZIP_DEFLATED
            zi.external_attr = 0o600 << 16
            z.writestr(zi, data)
        zi = zipfile.ZipInfo("docProps/thumbnail.jpeg", date_time=ZIP_TS)
        z.writestr(zi, thumb)
    return {"total_bytes": os.path.getsize(path),
            "member_order": [m[0] for m in members] + ["docProps/thumbnail.jpeg"]}


ODF_META = f"""<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta
 xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"
 xmlns:dc="http://purl.org/dc/elements/1.1/">
 <office:meta>
  <meta:initial-creator>{AUTHOR}</meta:initial-creator>
  <dc:creator>Petr Novak</dc:creator>
  <meta:creation-date>2024-05-02T08:11:00</meta:creation-date>
  <dc:date>2024-06-15T10:02:00</dc:date>
  <meta:print-date>2024-06-13T16:40:00</meta:print-date>
  <meta:printed-by>Jan Prochazka</meta:printed-by>
  <meta:generator>LibreOffice/7.6.4.1$Linux_X86_64 LibreOffice_project/</meta:generator>
  <meta:editing-cycles>14</meta:editing-cycles>
  <meta:editing-duration>PT41H11M</meta:editing-duration>
  <dc:title>Severin settlement memo</dc:title>
  <meta:user-defined meta:name="Matter">SEV-2024-0188</meta:user-defined>
  <meta:document-statistic meta:page-count="3" meta:word-count="914"/>
 </office:meta>
</office:document-meta>
"""

# The printer a document was last sent to names the office it was written in.
ODF_SETTINGS = """<?xml version="1.0" encoding="UTF-8"?>
<office:document-settings
 xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0">
 <office:settings><config:config-item-set config:name="ooo:view-settings">
  <config:config-item config:name="PrinterName" config:type="string">\
HP-LaserJet-M404-PRAHA-4F</config:config-item>
 </config:config-item-set></office:settings>
</office:document-settings>
"""

ODF_CONTENT = """<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
 xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:dc="http://purl.org/dc/elements/1.1/">
 <office:body><office:text>
  <text:tracked-changes><text:changed-region text:id="ct1"><text:deletion>
   <office:change-info><dc:creator>Petr Novak</dc:creator>
    <dc:date>2024-06-15T10:00:00</dc:date></office:change-info>
   <text:p>CZK 1,950,000</text:p>
  </text:deletion></text:changed-region></text:tracked-changes>
  <text:p>Settlement of CZK 1,200,000.</text:p>
  <office:annotation><dc:creator>Marie Dvorakova</dc:creator>
   <text:p>check this with legal</text:p></office:annotation>
 </office:text></office:body>
</office:document-content>
"""

ODF_MANIFEST = """<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">
 <manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="settings.xml" manifest:media-type="text/xml"/>
 <manifest:file-entry manifest:full-path="Thumbnails/thumbnail.png" manifest:media-type="image/png"/>
</manifest:manifest>
"""


def make_odt(path, thumb):
    """An OpenDocument package, whose vocabulary the engine does not read.

    `mimetype` has to come first and be stored rather than deflated, which is
    how a reader identifies an ODF package without unpacking it, and is also
    why rewriting this container is not just the OOXML path with other names.
    """
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        zi = zipfile.ZipInfo("mimetype", date_time=ZIP_TS)
        zi.compress_type = zipfile.ZIP_STORED
        z.writestr(zi, "application/vnd.oasis.opendocument.text")
        for name, data in [("META-INF/manifest.xml", ODF_MANIFEST),
                           ("content.xml", ODF_CONTENT), ("meta.xml", ODF_META),
                           ("settings.xml", ODF_SETTINGS)]:
            zi = zipfile.ZipInfo(name, date_time=ZIP_TS)
            zi.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(zi, data)
        zi = zipfile.ZipInfo("Thumbnails/thumbnail.png", date_time=ZIP_TS)
        z.writestr(zi, thumb)
    return {"total_bytes": os.path.getsize(path),
            "mimetype_first_and_stored": True}


def make_png_oriented(path):
    """A PNG as an editor leaves one after a phone photo: a camera's details in
    an eXIf block, an orientation that decides which way up it is shown, and a
    colour profile.

    The orientation and the profile are the never-remove rule in a format where
    the whole eXIf block is itself unusual enough to be worth dropping. Both
    pulls are real and the rule wins: a cleaned file that comes out on its side
    or in the wrong colours is a regression, not a protection.
    """
    w = h = 16
    raw = b"".join(b"\0" + bytes((x * 16 + y) % 256 for x in range(w)) for y in range(h))
    exif = E.build_exif(
        {0x010F: E.ascii_("Apple"), 0x0110: E.ascii_("iPhone 14 Pro"),
         0x0112: E.short_(6), 0x0131: E.ascii_("Adobe Photoshop 25.5")},
        {0x9003: E.ascii_(TS)}, {}, {}, b"")
    profile = icc_stub()
    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 0, 0, 0, 0))
    out += chunk(b"eXIf", exif)
    out += chunk(b"iCCP", b"ICC Profile\0\0" + zlib.compress(profile, 9))
    out += chunk(b"IDAT", zlib.compress(raw, 9))
    out += chunk(b"IEND", b"")
    open(path, "wb").write(out)
    return {"total_bytes": len(out), "orientation": 6, "icc_bytes": len(profile)}


# --------------------------------------------------------------------------- #
# SVG and MP4
# --------------------------------------------------------------------------- #

SVG = f"""<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:dc="http://purl.org/dc/elements/1.1/"
 xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
 xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape"
 xmlns:sodipodi="http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd"
 width="200" height="100" viewBox="0 0 200 100"
 inkscape:version="1.3.2 (091e20e, 2023-11-25)"
 sodipodi:docname="org-chart-INTERNAL-v4.svg">
 <metadata><rdf:RDF><cc:Work xmlns:cc="http://creativecommons.org/ns#">
  <dc:creator><cc:Agent><dc:title>{AUTHOR}</dc:title></cc:Agent></dc:creator>
  <dc:date>2024-06-14</dc:date>
 </cc:Work></rdf:RDF></metadata>
 <!-- A bleed background: anchored off the canvas, covering all of it. -->
 <rect x="-10" y="-10" width="220" height="120" fill="#6b7a8f"/>
 <g transform="translate(4,2)" inkscape:label="Layer 1">
  <rect x="10" y="20" width="180" height="60" fill="#8f7a6b"/>
 </g>
 <!-- Parked outside the viewBox: invisible on render, present in the file.
      It follows a closed transform group, which is what a counter that never
      came back down could not see past. -->
 <text x="40" y="400" font-size="10">superseded headcount plan: -18 FTE Q3</text>
</svg>"""


def make_svg(path):
    open(path, "w").write(SVG)
    return {"total_bytes": os.path.getsize(path),
            "offcanvas_text": "superseded headcount plan: -18 FTE Q3",
            "bleed_rect": "x=-10 y=-10 220x120, anchored off the canvas and covering it",
            "closed_transform_group_before_the_parked_text": True}


def qt_text_atom(typ, text, lang=0x15C7):
    """QuickTime international-text atom, the form a phone writes into moov/udta."""
    raw = text.encode("utf-8")
    return struct.pack(">I", 12 + len(raw)) + typ + struct.pack(">HH", len(raw), lang) + raw


def children(b, start, end):
    i = start
    while i + 8 <= end:
        sz = struct.unpack_from(">I", b, i)[0]
        if sz < 8:
            break
        yield i, sz, b[i + 4:i + 8]
        i += sz


def inject_udta(data, atoms):
    """Append atoms inside moov/udta, growing udta and moov.

    Safe only because ffmpeg places moov after mdat here: growing an atom that
    precedes mdat would shift the media and invalidate every stco offset.
    """
    top = {t: (o, sz) for o, sz, t in children(data, 0, len(data))}
    assert top[b"moov"][0] > top[b"mdat"][0], "moov precedes mdat; stco would break"
    moov_off, moov_sz = top[b"moov"]
    udta = next(((o, sz) for o, sz, t in children(data, moov_off + 8, moov_off + moov_sz)
                 if t == b"udta"), None)
    assert udta, "no moov/udta"
    udta_off, udta_sz = udta
    blob = b"".join(atoms)
    out = (data[:udta_off] + struct.pack(">I", udta_sz + len(blob))
           + data[udta_off + 4:udta_off + udta_sz] + blob + data[udta_off + udta_sz:])
    return out[:moov_off] + struct.pack(">I", moov_sz + len(blob)) + out[moov_off + 4:]


def make_mp4(path):
    subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi",
         "-i", "testsrc=s=160x120:d=1", "-c:v", "libx264", "-pix_fmt", "yuv420p",
         "-metadata", "location=+50.0755+014.4378/",
         "-metadata", "location-eng=+50.0755+014.4378/",
         "-metadata", "make=Apple", "-metadata", "model=iPhone 14 Pro",
         "-metadata", "artist=" + AUTHOR,
         "-metadata", "comment=do not publish",
         "-metadata", "creation_time=2024-06-14T19:42:07Z", path],
        check=True, capture_output=True)
    data = inject_udta(open(path, "rb").read(), [
        qt_text_atom(b"\xa9xyz", "+50.0755+014.4378/"),
        qt_text_atom(b"\xa9mak", "Apple"),
        qt_text_atom(b"\xa9mod", "iPhone 14 Pro"),
        qt_text_atom(b"\xa9dat", "2024-06-14T19:42:07Z"),
    ])
    open(path, "wb").write(data)
    return {"total_bytes": len(data),
            "mvhd_created_1904_epoch": 1718394127 + 2082844800}


def make_mov_ios(path):
    """A `.mov` carrying iOS-style metadata.

    `-movflags use_metadata_tags` makes ffmpeg write a `keys` dictionary and a
    numbered `ilst`, which is the form a recent iPhone uses: the values carry
    no four-byte type of their own, so a handler that only knows atom types
    finds nothing at all here. QuickTime's `meta` also has no version word,
    unlike the ISO one, which is the second way to miss the whole box.
    """
    subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi",
         "-i", "testsrc=s=160x120:d=1", "-c:v", "libx264", "-pix_fmt", "yuv420p",
         "-movflags", "use_metadata_tags",
         "-metadata", "com.apple.quicktime.make=Apple",
         "-metadata", "com.apple.quicktime.model=iPhone 14 Pro",
         "-metadata", "com.apple.quicktime.software=17.4.1",
         "-metadata", "com.apple.quicktime.location.ISO6709=+50.0755+014.4378/",
         "-metadata", "com.apple.quicktime.creationdate=2024-06-14T19:42:07+0200",
         "-metadata", "com.apple.photos.originating.signature=" + MOV_SIGNATURE,
         path],
        check=True, capture_output=True)
    data = open(path, "rb").read()
    assert b"keys" in data and b"mdta" in data, "ffmpeg did not write a keys dictionary"
    return {"total_bytes": len(data), "keys_box": True}


def make_mp4_truncated(src, path, missing=15):
    """`clip.mp4` with its last bytes gone, which is what an interrupted copy
    looks like. `moov` is last, so the box holding every field is the one cut
    short: the atoms are still in the bytes and no parser reaches them."""
    data = open(src, "rb").read()[:-missing]
    open(path, "wb").write(data)
    return {"total_bytes": len(data), "bytes_missing": missing}


# --------------------------------------------------------------------------- #

DECLARED = {
    "photo-phone.jpg": {
        "identity": ["EXIF Artist=" + AUTHOR, "EXIF Copyright", "EXIF ImageDescription",
                     "IPTC 2:80 By-line=" + AUTHOR, "XMP dc:creator"],
        "device": ["Make=Apple", "Model=iPhone 14 Pro", "Software=17.4.1",
                   "BodySerialNumber=" + SERIAL_BODY, "LensSerialNumber=" + SERIAL_LENS,
                   "LensModel", "MakerNote (opaque, carries the body serial again)"],
        "location": [f"GPSLatitude={LAT} N", f"GPSLongitude={LON} E",
                     "GPSAltitude=217.40", "GPSTimeStamp", "GPSDateStamp=2024:06:14",
                     "IPTC 2:90 City=Praha", "IPTC 2:101 Country"],
        "time": ["DateTimeOriginal=" + TS, "DateTimeDigitized", "DateTime", "XMP xmp:MetadataDate"],
        "correlation": ["XMP xmpMM:DocumentID", "XMP xmpMM:InstanceID",
                        "XMP xmpMM:OriginalDocumentID", "EXIF ImageUniqueID (0xA420)"],
        "hidden_payload": ["IFD1 thumbnail is 160x120 testsrc, visibly NOT a scaled "
                           "copy of the 320x240 main image: the uncropped original"],
        "never_remove": ["APP2 ICC_PROFILE (0-tag stub)"],
    },
    "photo-codepage.jpg": {
        "identity": [f"EXIF Artist={CP_AUTHOR}, in Windows-1252 rather than UTF-8",
                     "EXIF Copyright, same encoding",
                     "EXIF ImageDescription with German accents"],
        "device": ["EXIF Make=Müller Zeichenbüro, in Windows-1252"],
        "_contract": "Nothing in the file declares the encoding, because EXIF says these "
                     "fields are ASCII and they are not. The report has to show the "
                     "letters rather than replacement characters, and has to say that "
                     "the encoding was a fallback, because a Central European codepage "
                     "would give different accents from the same bytes.",
    },
    "screenshot.png": {
        "identity": ["tEXt Author=" + AUTHOR, "XMP dc:creator"],
        "device": ["tEXt Software=GNOME Screenshot 45.1", "tEXt Comment names the host",
                   "eXIf Software"],
        "time": ["tIME=2024-06-14T17:42:07Z", "eXIf DateTimeOriginal"],
        "correlation": ["iTXt XML:com.adobe.xmp carries DocumentID/InstanceID"],
    },
    "photo-oriented.png": {
        "device": ["eXIf Make=Apple", "eXIf Model=iPhone 14 Pro",
                   "eXIf Software=Adobe Photoshop 25.5"],
        "time": ["eXIf DateTimeOriginal=" + TS],
        "never_remove": ["eXIf Orientation=6, which decides which way up the image is "
                         "shown: it survives every profile, including the ones that drop "
                         "the rest of the block to match what common tools leave",
                         "iCCP colour profile, untouched by every profile"],
        "_contract": "The whole eXIf block is unusual in a PNG and worth dropping, and "
                     "the orientation inside it must not go with it. Dropping it turns "
                     "the picture on its side, and reporting it as preserved while "
                     "dropping it is worse than either.",
    },
    "report-redacted.pdf": {
        "identity": ["/Author=" + AUTHOR, "XMP dc:creator"],
        "device": ["/Creator=Microsoft Word 2021", "/Producer=Acrobat Distiller 23.0"],
        "time": ["/CreationDate", "/ModDate"],
        "correlation": ["trailer /ID pair=" + PDF_FILE_ID, "XMP xmpMM:DocumentID",
                        "XMP xmpMM:InstanceID"],
        "redaction_integrity": ["text drawn, then covered by a filled rect; still "
                                "extractable: " + SECRET],
        "hidden_payload": ["/OpenAction JavaScript", "/EmbeddedFiles notes.txt",
                           "page /Annots FileAttachment payroll-draft.csv, which the "
                           "catalog name tree does not mention",
                           "page /AA /O JavaScript, which runs on page open"],
    },
    "report-incremental.pdf": {
        "history": ["incremental update: v2 overwrites obj 4 and 6, v1 remains in the "
                    "earlier revision and is fully recoverable (CZK 1,950,000)"],
        "identity": ["/Author in the v1 Info dict survives into the v2 file"],
    },
    "signed.pdf": {
        "never_remove": ["AcroForm /SigFlags 3 with a /Sig field. Any metadata edit "
                         "invalidates a real signature: detect and refuse. The /Contents "
                         "here is a stub, not real CAdES."],
        "identity": ["/Author", "/Sig /Name"],
    },
    "nested.pdf": {
        "_contract": "Twenty thousand matched, correctly indexed nested arrays. Every "
                     "bracket pairs and the xref is right, so this is hostile rather than "
                     "broken: a recursive parser overflows its stack on it and the process "
                     "aborts, which a folder run cannot survive. The contract is a refusal "
                     "naming the depth, decided before the file is parsed.",
    },
    "encrypted.pdf": {
        "identity": ["/Author in the Info dict, which in a real encrypted file would be "
                     "ciphertext rather than readable"],
        "never_remove": ["/Encrypt declaring the Standard handler, revision 3. The /O and "
                         "/U strings are stubs, as in signed.pdf. Detect and refuse: the "
                         "strings and streams are ciphertext and the trailer /ID is an "
                         "input to the key, so any rewrite leaves a file nothing opens."],
        "_contract": "A refusal on inspect and on clean, with nothing reported out of the "
                     "file, because whatever it holds is ciphertext.",
    },
    "letter.docx": {
        "identity": ["dc:creator=" + AUTHOR, "cp:lastModifiedBy=Petr Novak",
                     "Company=Severin Holding a.s.", "Manager=Jan Prochazka",
                     "Template=sev-internal-memo.dotx"],
        "time": ["dcterms:created=2024-05-02", "dcterms:modified=2024-06-15",
                 "cp:lastPrinted=2024-06-13", "TotalTime=2471 editing minutes"],
        "correlation": ["w15:docId=" + WORD_DOCID, "6x w:rsid save IDs"],
        "history": ["w:ins and w:del tracked changes naming both authors and the "
                    "superseded figure", "comments.xml with author and initials",
                    "cp:revision=14"],
        "hidden_payload": ["docProps/thumbnail.jpeg (embedded preview)"],
        "structural_fingerprint": ["every ZIP member stamped 2019-03-14 09:26:52 (DOS time has 2-second\n                                    granularity, so odd seconds are unrepresentable)",
                                   "non-alphabetical Word-like member order"],
    },
    "diagram.svg": {
        "identity": ["dc:creator/cc:Agent=" + AUTHOR],
        "device": ["inkscape:version=1.3.2"],
        "time": ["dc:date=2024-06-14"],
        "correlation": ["sodipodi:docname=org-chart-INTERNAL-v4.svg"],
        "hidden_payload": ["<text y=\"400\"> parked outside the 0 0 200 100 viewBox, and "
                           "after a closed <g transform> group"],
        "never_remove": ["the bleed <rect x=\"-10\" y=\"-10\" width=\"220\" height=\"120\">, "
                         "which is anchored off the canvas and covers all of it: visible "
                         "artwork, not hidden payload, and not removable by any profile",
                         "the <rect> inside the transform group"],
    },
    "clip.mp4": {
        "location": ["moov/udta \\xa9xyz = +50.0755+014.4378/ (hand-injected; the form "
                     "a phone writes)",
                     "3GPP loci box, written by ffmpeg from -metadata location"],
        "device": ["moov/udta \\xa9mak=Apple", "moov/udta \\xa9mod=iPhone 14 Pro",
                   "ilst \\xa9too=Lavf... encoder string"],
        "identity": ["ilst \\xa9ART=" + AUTHOR, "ilst \\xa9cmt=do not publish"],
        "time": ["mvhd creation_time=3801238927 (seconds since 1904-01-01, binary)",
                 "moov/udta \\xa9dat=2024-06-14T19:42:07Z"],
    },
    "report.odt": {
        "identity": ["meta:initial-creator=" + AUTHOR, "dc:creator=Petr Novak",
                     "meta:printed-by=Jan Prochazka", "dc:title",
                     "meta:user-defined Matter=SEV-2024-0188",
                     "settings.xml PrinterName=HP-LaserJet-M404-PRAHA-4F"],
        "device": ["meta:generator=LibreOffice/7.6.4.1"],
        "time": ["meta:creation-date", "dc:date", "meta:print-date"],
        "history": ["meta:editing-cycles=14", "meta:editing-duration=PT41H11M",
                    "text:tracked-changes keeping the deleted figure and its author",
                    "office:annotation naming " + AUTHOR],
        "hidden_payload": ["Thumbnails/thumbnail.png, a preview of the first page"],
        "_contract": "The ODF vocabulary is not read yet, and reading this package with "
                     "the OOXML part names finds no properties at all, which in a report "
                     "is indistinguishable from a document that has none. So the contract "
                     "is a refusal naming the format on inspect and on clean, not a field "
                     "count and not a clean.",
    },
    "clip-ios.mov": {
        "location": ["moov/meta keys com.apple.quicktime.location.ISO6709 "
                     "= +50.0755+014.4378/, value in a numbered ilst entry"],
        "device": ["com.apple.quicktime.make=Apple",
                   "com.apple.quicktime.model=iPhone 14 Pro",
                   "com.apple.quicktime.software=17.4.1",
                   "ilst encoder=Lavf... string"],
        "time": ["com.apple.quicktime.creationdate=2024-06-14T19:42:07+0200",
                 "mvhd creation_time"],
        "correlation": ["com.apple.photos.originating.signature=" + MOV_SIGNATURE,
                        "the keys dictionary itself, which names every field the "
                        "recorder wrote even once the values are gone"],
    },
    "clip-truncated.mp4": {
        "location": ["clip.mp4's \\xa9xyz and loci bytes, present but unreachable"],
        "device": ["clip.mp4's \\xa9mak and \\xa9mod, present but unreachable"],
        "identity": ["clip.mp4's ilst artist and comment, present but unreachable"],
        "_contract": "An interrupted copy: moov is cut short, so the declared fields "
                     "are in the bytes and no parser reaches them. The only honest "
                     "answers are a refusal on inspect and a refusal on clean. A field "
                     "count here, or a clean that reports nothing changed, is a false "
                     "clean on a file that still holds everything.",
    },
    "_filesystem": {
        "filesystem": ["mtime and atime set to 2024-06-14 19:42:07 UTC on every fixture",
                       "user.figmun.test xattr on photo-phone.jpg",
                       "photo-phone.jpg.Identifier sidecar standing in for the Windows "
                       "Zone.Identifier ADS, which cannot exist on this filesystem"],
    },
}


def main():
    os.makedirs(OUT, exist_ok=True)
    facts = {}
    jp = make_jpeg(f"{OUT}/photo-phone.jpg")
    thumb = jp.pop("_thumb")
    facts["photo-phone.jpg"] = jp
    facts["photo-codepage.jpg"] = make_jpeg_codepage(f"{OUT}/photo-codepage.jpg")
    facts["screenshot.png"] = make_png(f"{OUT}/screenshot.png")
    facts["photo-oriented.png"] = make_png_oriented(f"{OUT}/photo-oriented.png")
    facts["report-redacted.pdf"] = make_pdf_redacted(f"{OUT}/report-redacted.pdf")
    facts["report-incremental.pdf"] = make_pdf_incremental(f"{OUT}/report-incremental.pdf")
    facts["signed.pdf"] = make_pdf_signed(f"{OUT}/signed.pdf")
    facts["nested.pdf"] = make_pdf_nested(f"{OUT}/nested.pdf")
    facts["encrypted.pdf"] = make_pdf_encrypted(f"{OUT}/encrypted.pdf")
    facts["letter.docx"] = make_docx(f"{OUT}/letter.docx", thumb)
    facts["report.odt"] = make_odt(f"{OUT}/report.odt", thumb)
    facts["diagram.svg"] = make_svg(f"{OUT}/diagram.svg")
    facts["clip.mp4"] = make_mp4(f"{OUT}/clip.mp4")
    facts["clip-ios.mov"] = make_mov_ios(f"{OUT}/clip-ios.mov")
    facts["clip-truncated.mp4"] = make_mp4_truncated(
        f"{OUT}/clip.mp4", f"{OUT}/clip-truncated.mp4")

    # Filesystem layer.
    open(f"{OUT}/photo-phone.jpg.Identifier", "w").write(
        "[ZoneTransfer]\nZoneId=3\nReferrerUrl=https://mail.example.cz/\n"
        "HostUrl=https://mail.example.cz/attachment/4471902\n")
    stamp = 1718394127  # 2024-06-14 19:42:07 UTC
    for f in sorted(os.listdir(OUT)):
        os.utime(f"{OUT}/{f}", (stamp, stamp))
    try:
        os.setxattr(f"{OUT}/photo-phone.jpg", b"user.figmun.test",
                    b"declared-fixture")
        facts["_filesystem"] = {"xattr_set": True}
    except OSError as e:
        facts["_filesystem"] = {"xattr_set": False, "reason": str(e)}

    man = {"generator": "fixtures/generate.py", "note":
           "Declared ground truth for the inspect phase. Regenerate rather than edit.",
           "fixtures": {k: {"classes": DECLARED[k], "observed": facts.get(k, {})}
                        for k in DECLARED}}
    with open("manifest.json", "w") as fh:
        json.dump(man, fh, indent=2, sort_keys=True)

    for name in sorted(f for f in DECLARED if not f.startswith("_")):
        cls = ", ".join(sorted(k for k in DECLARED[name] if not k.startswith("_")))
        print(f"  {name:26} {facts[name]['total_bytes']:>7} B   {cls}")
    print(f"\n  manifest.json written, {len(DECLARED)} entries")


if __name__ == "__main__":
    main()
