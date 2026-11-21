#!/bin/sh
# Check the claims in the README's trust posture, rather than asserting them.
#
# Three of them are mechanically checkable and this checks all three:
#
#   1. Nothing in any dependency closure can do networking.
#   2. The engine and the command line import no socket symbols at all, so those
#      binaries cannot open one. The window does, and this says why rather than
#      hiding it: a display stack talks over a socket and an X11 client can be
#      pointed at a remote display.
#   3. No dependency is under a licence that would contradict MPL-2.0 with a
#      separately-licensed layer linking it.
#
# Exits non-zero on anything unexpected, so it belongs in CI.
set -eu
cd "$(dirname "$0")/.."
fail=0
say() { printf '%s\n' "$*"; }

say "== networking crates in each dependency closure =="
cargo metadata --format-version 1 >/tmp/fm-meta.json
python3 - <<'PY' || fail=1
import json, re, sys
d = json.load(open("/tmp/fm-meta.json"))
pkgs = {p["id"]: p for p in d["packages"]}
res = {n["id"]: n for n in d["resolve"]["nodes"]}

def closure(root):
    seen, stack = set(), [root]
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        stack.extend(res[i]["dependencies"])
    return seen

# Anything that speaks a network protocol or resolves a name.
#
# This is a denylist, so it only knows what it has been told, and a crate can
# reach the network through std with no dependency of its own. That gap is the
# symbol check below, which reads the finished binaries rather than the
# manifest. Display, clipboard and D-Bus stacks are deliberately absent here
# and reported separately: they open sockets, locally, and that is what a
# display server is.
NET = re.compile(
    r"^(reqwest|hyper|h2|ureq|curl|isahc|surf|native-tls|openssl|rustls|"
    r"trust-dns|hickory|quinn|tungstenite|ftp|smtp|lettre|aws-|azure_|google-)",
    re.I,
)
members = {pkgs[m]["name"]: m for m in d["workspace_members"]}
bad = 0
for name in sorted(members):
    hits = sorted({pkgs[i]["name"] for i in closure(members[name]) if NET.match(pkgs[i]["name"])})
    print(f"   {name:10} {'none' if not hits else hits}")
    if hits:
        bad = 1
sys.exit(bad)
PY

say ""
say "== socket symbols in the release artefacts =="
# A check that cannot run is not a check that passed. Each of these used to be
# a quiet skip or a silent zero, which is the same shape of lie as a cleaner
# that reports a file as clean because it could not read it.
bin="${CARGO_TARGET_DIR:-target}/release"
if ! command -v nm >/dev/null 2>&1; then
    say "   nm is not installed, so nothing here can be read: install binutils"
    fail=1
elif [ ! -x "$bin/fm-cli" ]; then
    say "   $bin/fm-cli is not built: run cargo build --release --workspace first"
    fail=1
else
    sockets='" (socket|connect|sendto|recvfrom|getaddrinfo|gethostby)"'
    # fm-runner is a library, and the artefact that matters is the shared
    # object the Android app loads, which is a real link with real imports.
    for artefact in fm-cli libfm_runner.so; do
        if [ ! -e "$bin/$artefact" ]; then
            say "   $artefact is missing, so its symbols were not read"
            fail=1
            continue
        fi
        imports=$(nm -D --undefined-only "$bin/$artefact" 2>/dev/null | wc -l)
        if [ "$imports" -eq 0 ]; then
            # No imports at all means nm read nothing useful: a static binary,
            # a stripped one, or a format it does not understand. Reporting
            # "no socket symbols" from that would be a false pass.
            say "   $artefact: nm found no imported symbols at all, so this says nothing"
            fail=1
            continue
        fi
        found=$(nm -D --undefined-only "$bin/$artefact" 2>/dev/null |
            grep -cE " (socket|connect|sendto|recvfrom|getaddrinfo|gethostby)" || true)
        if [ "$found" -eq 0 ]; then
            say "   $artefact: $imports imports, none of them a socket call"
        else
            say "   $artefact: FAIL, imports $found socket symbol(s)"
            fail=1
        fi
    done
    if [ -x "$bin/fm-gui" ]; then
        found=$(nm -D --undefined-only "$bin/fm-gui" 2>/dev/null |
            grep -cE " (socket|connect|getaddrinfo)" || true)
        say "   fm-gui: imports $found socket symbol(s), which is expected:"
        say "           a display server is reached over a Unix socket, and the X11"
        say "           client in the clipboard stack can target a remote display."
        say "           The check that matters for the window is the crate audit above."
    else
        say "   fm-gui is not built, so its symbols were not read"
        fail=1
    fi
fi

say ""
say "== licences across every closure =="
python3 - <<'PY' || fail=1
import json, sys
d = json.load(open("/tmp/fm-meta.json"))
pkgs = {p["id"]: p for p in d["packages"]}
res = {n["id"]: n for n in d["resolve"]["nodes"]}

def closure(root):
    seen, stack = set(), [root]
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        stack.extend(res[i]["dependencies"])
    return seen

members = [m for m in d["workspace_members"]]
everything = set()
for m in members:
    everything |= closure(m)

# A strong copyleft licence with no permissive alternative would stop a
# separately-licensed layer from linking the engine. An unstated licence is
# worse than a bad one, because it cannot be reasoned about at all.
problems = []
for i in sorted(everything):
    p = pkgs[i]
    lic = p.get("license")
    if not lic:
        problems.append(f"{p['name']} {p['version']}: no licence stated")
        continue
    alternatives = [a.strip() for a in lic.replace("/", " OR ").split(" OR ")]
    if all(("GPL" in a.upper() and "LGPL" not in a.upper()) for a in alternatives):
        problems.append(f"{p['name']} {p['version']}: {lic}")
print(f"   {len(everything)} crates across the whole workspace")
for line in problems:
    print("   FAIL", line)
sys.exit(1 if problems else 0)
PY

say ""
if [ "$fail" -eq 0 ]; then
    say "trust posture: every claim that can be checked, checks out"
else
    say "trust posture: FAILED"
fi
exit "$fail"
