# Design notes

Why the options are shaped the way they are, and what was decided along the
way. [`../README.md`](../README.md) is the shorter account; this is the
reasoning behind it, kept because the opposite of each decision looks equally
sensible until you know the reason.

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

That promise is enforced rather than intended: a clean whose diff names a field
the report did not is refused, and nothing is written. Consequences of cleaning
are exempt and say so one by one in the code, because sample offsets corrected
for removed bytes were never a field anyone could have been shown. The check
earned its place immediately: a photograph was losing its EXIF version field
with no mention of it in either the report or the diff, because the reader
skipped tags it did not recognise while the writer dropped them anyway.

## What can be done to each

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

## Blending in

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

## The profile file

The table of what each built-in profile does to each class is in
[`../README.md`](../README.md#profiles). What it does not say is why `uniform`
exists: it is the one that makes two files from different people
indistinguishable, and also the one most likely to look odd to a recipient, so
it is offered and not defaulted to.

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

(`fm-cli profile strict --save mine.toml` writes exactly that, and `fm-cli clean
--profile mine.toml` reads it back.)

Cells are validated rather than trusted: `identity = "coarsen-day"` is rejected
at load, because coarsening a name means nothing and a profile that silently did
nothing would be worse than one that refused.

The `[runner]` block is the mobile sweep's whole configuration, which is why the
desktop writes it even though the desktop does not use it.

## The window

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

## The phone

A photograph is geotagged the moment it is taken, and a user who has to remember
to clean it before sending it will forget. So the phone's product is a
**background runner**: it sweeps the folders it was pointed at, on a schedule,
under one profile, and says what it did. The profile file is the one the desktop
writes, unchanged, so one profile can be carried between them.

The runner has no file paths in it, which is the decision the mobile design turns
on: since scoped storage an Android app reaches the camera roll through document
URIs, not paths. The reasoning, the JNI surface, and why **iOS cannot have the
runner at all**, are in [`mobile/README.md`](../mobile/README.md).

## Shell integration, and no browser extension

Not built. Explorer, Nautilus and Dolphin entries and macOS Quick Actions all
shell out to `fm-cli clean`, so they are packaging rather than engineering, and
follow the path already proven by Figura Obscura.

**No browser extension.** Transparent upload interception is not buildable: MV3's
`declarativeNetRequest` cannot touch request bodies, blocking `webRequest` is gone,
and `requestBody` is read-only even in Firefox. The MAIN-world JS-patching workaround
breaks on every site redesign, which is a maintenance tax this project exists to
avoid.

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
4. **What a second clean to the same place should do.** Cleaning beside the
   originals twice under one profile writes the identical file again, which is
   fine. Under a different profile the destination holds something this run did
   not produce, and that is refused rather than overwritten, because the same
   check is what stops an unrelated file of the same name being destroyed. The
   alternative is a new name, which trades a refusal the user has to act on for
   clutter they may not notice. The refusal is the safe default and not
   obviously the kind one.
5. **What an audit trail is allowed to contain.** Decisions and counts can be
   recorded safely. Filenames and field values cannot, because the record then
   becomes a more sensitive document than the ones it audits. Settling this
   before the feature exists is cheaper than settling it afterwards.
