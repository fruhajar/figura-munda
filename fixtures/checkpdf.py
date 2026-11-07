#!/usr/bin/env python3
"""Independent structural check of a PDF, sharing no code with the engine.

The Rust side reads its own output back through the same library that wrote it,
which proves the library is self-consistent and nothing else. This parses the
cross-reference table by hand and asserts every offset in it points at the
object it claims to, which is the failure a self-consistent library would hide
and which makes a file every other reader rejects.

    python3 checkpdf.py FILE...             any PDF: is it structurally sound
    python3 checkpdf.py --cleaned FILE...   output: and does it hold the
                                            promises cleaning makes

The second mode is stricter because cleaned output has promises an arbitrary
input does not: one revision, no chain to a previous one, and a trailer that
does not still carry the keys of the cross-reference stream it may have been
loaded from. Running it over an input fixture would fail on the incremental
one, which has a previous revision on purpose.
"""
import re
import sys


def fail(path, message):
    print(f"  \033[31mFAIL\033[0m {path}: {message}")
    return 1


def check(path, cleaned=False):
    data = open(path, "rb").read()
    problems = 0

    if not data.startswith(b"%PDF-"):
        return fail(path, "no %PDF- header")

    starts = [m.start() for m in re.finditer(rb"startxref", data)]
    if not starts:
        return fail(path, "no startxref")
    # A cleaned file should have exactly one revision.
    revisions = len(starts)

    tail = data[starts[-1]:]
    m = re.match(rb"startxref\s+(\d+)", tail)
    if not m:
        return fail(path, "startxref has no offset after it")
    xref_at = int(m.group(1))
    if xref_at >= len(data):
        return fail(path, f"startxref points past the end of the file ({xref_at})")

    if not data[xref_at:].startswith(b"xref"):
        # A cross-reference stream is legal PDF and this checker cannot read
        # one. Saying SKIP quietly would mean the files whose structure is
        # least verified are the ones that go unchecked, which is how a
        # malformed trailer survived here once already, so it is reported
        # loudly and counts against the run. Cleaned output is always a table.
        print(
            f"  \033[33mUNCHECKED\033[0m {path}: cross-reference stream, "
            "which this checker cannot read"
        )
        return 1

    # xref\n <first> <count>\n then <count> twenty-byte entries.
    body = data[xref_at + 4:]
    pos = 0
    entries = {}
    while True:
        head = re.match(rb"\s*(\d+)\s+(\d+)\s*", body[pos:])
        if not head:
            break
        first, count = int(head.group(1)), int(head.group(2))
        pos += head.end()
        for i in range(count):
            entry = body[pos:pos + 20]
            if len(entry) < 18:
                return fail(path, "cross-reference section is truncated")
            fields = entry.split()
            if len(fields) < 3:
                return fail(path, f"malformed cross-reference entry: {entry!r}")
            offset, _generation, kind = int(fields[0]), int(fields[1]), fields[2]
            if kind == b"n":
                entries[first + i] = offset
            pos += 20
        if body[pos:pos + 7].strip().startswith(b"trailer"):
            break

    if not entries:
        return fail(path, "cross-reference table lists no objects")

    for number, offset in sorted(entries.items()):
        if offset >= len(data):
            problems += fail(path, f"object {number} offset {offset} is past the end")
            continue
        here = data[offset:offset + 40]
        if not re.match(rb"\s*%d\s+\d+\s+obj" % number, here):
            problems += fail(
                path, f"object {number} offset {offset} points at {here[:24]!r}"
            )

    trailer = data.rfind(b"trailer")
    if trailer == -1:
        problems += fail(path, "no trailer")
    else:
        body = data[trailer:trailer + 600]
        if b"/Root" not in body:
            problems += fail(path, "trailer names no /Root")
        # A table's trailer must not carry a cross-reference stream's keys. A
        # document loaded from a stream keeps that stream's dictionary, and
        # writing a table without clearing it produces exactly this.
        for key in (b"/Type/XRef", b"/Type /XRef", b"/W[", b"/W [") if cleaned else ():
            if key in body:
                problems += fail(
                    path, f"table trailer carries the stream key {key.decode()}"
                )
        if cleaned and b"/Prev" in body:
            problems += fail(path, "trailer points at a previous revision")

    if cleaned and revisions > 1:
        problems += fail(path, f"{revisions} revisions; cleaning should leave one")

    if not problems:
        print(
            f"  \033[32mPASS\033[0m {path}: {len(entries)} objects, "
            f"{revisions} revision(s), every offset lands on its object"
        )
    return problems


def main(argv):
    cleaned = "--cleaned" in argv
    paths = [a for a in argv if not a.startswith("--")]
    if not paths:
        print(__doc__)
        return 2
    problems = sum(check(p, cleaned) for p in paths)
    print()
    if problems:
        print(f"{problems} problem(s)")
    else:
        print(f"{len(paths)} file(s) structurally sound")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
