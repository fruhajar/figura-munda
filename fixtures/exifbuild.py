"""Minimal TIFF/EXIF IFD writer. Offsets are relative to the TIFF header start."""
import struct

BYTE, ASCII, SHORT, LONG, RATIONAL, UNDEF, SRATIONAL = 1, 2, 3, 4, 5, 7, 10


def ascii_(s):
    return ASCII, len(s) + 1, s.encode("latin-1") + b"\0"


def short_(*v):
    return SHORT, len(v), b"".join(struct.pack("<H", x) for x in v)


def long_(*v):
    return LONG, len(v), b"".join(struct.pack("<I", x) for x in v)


def rational_(*pairs):
    return RATIONAL, len(pairs), b"".join(struct.pack("<II", n, d) for n, d in pairs)


def undef_(b):
    return UNDEF, len(b), b


def byte_(b):
    return BYTE, len(b), b


def build_ifd(entries, ifd_start, next_ifd=0):
    """entries: {tag: (tcode, count, raw)}. Returns (bytes, total_len)."""
    n = len(entries)
    data_base = ifd_start + 2 + 12 * n + 4
    table, data = struct.pack("<H", n), b""
    for tag in sorted(entries):
        tcode, count, raw = entries[tag]
        if len(raw) <= 4:
            table += struct.pack("<HHI", tag, tcode, count) + raw.ljust(4, b"\0")
        else:
            table += struct.pack("<HHI", tag, tcode, count)
            table += struct.pack("<I", data_base + len(data))
            data += raw + (b"\0" if len(raw) % 2 else b"")
    table += struct.pack("<I", next_ifd)
    return table + data, len(table) + len(data)


def ifd_len(entries):
    return build_ifd(entries, 0)[1]


def build_exif(ifd0, exif_ifd, gps_ifd, ifd1, thumb):
    """Lay out IFD0 -> ExifIFD -> GPS -> IFD1 -> thumbnail, then fix up pointers.

    Entry-table length is independent of the pointer values it holds (LONG is
    inline), so sizes can be measured in a first pass and offsets assigned in a
    second.
    """
    ifd0 = dict(ifd0)
    ifd0[0x8769] = long_(0)  # ExifIFD pointer, patched below
    ifd0[0x8825] = long_(0)  # GPS IFD pointer
    off0 = 8
    len0 = ifd_len(ifd0)
    off_exif = off0 + len0
    len_exif = ifd_len(exif_ifd)
    off_gps = off_exif + len_exif
    len_gps = ifd_len(gps_ifd)
    off_ifd1 = off_gps + len_gps

    ifd1 = dict(ifd1)
    ifd1[0x0201] = long_(0)
    ifd1[0x0202] = long_(len(thumb))
    len1 = ifd_len(ifd1)
    off_thumb = off_ifd1 + len1
    ifd1[0x0201] = long_(off_thumb)

    ifd0[0x8769] = long_(off_exif)
    ifd0[0x8825] = long_(off_gps)

    out = b"II" + struct.pack("<HI", 42, off0)
    out += build_ifd(ifd0, off0, next_ifd=off_ifd1)[0]
    out += build_ifd(exif_ifd, off_exif)[0]
    out += build_ifd(gps_ifd, off_gps)[0]
    out += build_ifd(ifd1, off_ifd1)[0]
    out += thumb
    return out
