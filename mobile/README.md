# Figura Munda on a phone

A photograph is geotagged the moment it is taken, and a user who has to remember
to clean it before sending it will forget. So the phone's product is not a
button, it is a **background runner**: it sweeps the folders it was pointed at,
on a schedule, under one profile, and says what it did.

```
mobile/
  runner/    the sweep, in Rust. Tested, and the tests run on any machine.
  android/   the host: WorkManager, the folder grant, the JNI bridge.
```

## What is built and tested

`runner/` is real and covered by 32 tests that run with `cargo test`, including
sweeps over the generated fixtures. It contains:

- **`sweep::due`** decides whether a sweep is owed at all: enabled, a folder
  chosen, the interval elapsed, and the battery above a floor unless charging. A
  privacy tool that flattens the battery gets uninstalled, which protects
  nobody.
- **`decide`** holds the per-file decision, with no file system in it: whether
  to look at a file, what cleaning it produces, and what to write down.
- **`journal`** records what has already been done, keyed by whatever the host
  calls a file, with the size and modification time the runner left behind. That
  is how it recognises its own work: the camera writes `IMG_0001.jpg` and the
  runner cleans it in place, so the name is identical before and after.
- **`fsdrive`** is the driver for hosts that have real paths.

Four behaviours in there are load-bearing and each has a test:

1. **A file written seconds ago is left alone.** A camera, a messaging app or a
   download may still be writing it, and cleaning a half-written file corrupts
   it. Thirty seconds of quiet is cheap when sweeps are hours apart.
2. **The journal is written after every file, not at the end.** Android kills
   background work whenever it likes. A journal that only existed at the end
   would mean every sweep starting over.
3. **A file that will not verify is never written.** On a desktop the user can
   compare against the original; on a phone there is no original left and nobody
   watching, so a run that cannot prove it worked must not touch the only copy.
4. **A failing file is given up on after three attempts.** Otherwise one
   malformed file is one failure in every sweep forever.

The profile file is the one the desktop writes, unchanged, so a profile can be
carried between the two and a file cleaned on either comes out identical.

## Why the runner has no file paths in it

This is the decision the whole mobile design turns on.

Since scoped storage, an Android app cannot modify arbitrary files under `DCIM`.
There are three ways in and only one of them is honest:

| Route | Verdict |
|---|---|
| `MANAGE_EXTERNAL_STORAGE` | Gives real paths, and is exactly the permission Google restricts on the Play Store to file managers and backup tools. Asking for whole-device file access is also the opposite of what this tool claims to be. **No.** |
| `MediaStore` writes | Needs per-item user consent for files the app did not create, which a background sweep cannot ask for. **No.** |
| Storage Access Framework tree grant | The user picks a folder once; the app holds persistent read and write access to it across reboots. Minimal, revocable, visible in system settings. **Yes.** |

A tree grant hands out **document URIs**, not paths, and they are opened through
the platform's content resolver. A runner built on `std::fs` would work on a
desktop and be unable to touch a photograph on any phone sold in the last five
years.

So the listing, reading and writing belong to the host, and `decide` holds the
part worth testing. `fsdrive` and the Android app are two drivers over one set
of decisions, and neither duplicates the other.

## The JNI surface

`runner/src/android.rs`, compiled only for `target_os = "android"`. A session
holds the profile and the journal so that a sweep over a thousand photographs
does not re-parse its own bookkeeping a thousand times.

| Rust | Called by the host to |
|---|---|
| `open(profileToml, journalJson) -> handle` | start a sweep |
| `due(handle, charging, battery, now) -> String` | ask if one is owed; empty means yes, otherwise the sentence to show |
| `plan(handle, item, now) -> int` | decide whether this file needs reading at all |
| `resolve(handle, item, bytes) -> ByteArray` | clean it; empty means nothing to write |
| `outcome(handle) -> String` | why, when `resolve` returned nothing |
| `record(handle, item, wroteBytes, wroteMtime, now)` | write down what the host actually wrote |
| `journal(handle) -> String` | the JSON to persist |
| `report(handle) -> String` | the line for the notification |
| `close(handle)` | finish |

`plan` returning "no need to read this" before any bytes are read is what keeps
a sweep cheap: deciding from the size and modification time that nothing changed
is the difference between a sweep the user never notices and one that reads a
thousand photographs to learn nothing.

## What is not built

**None of `android/` has been compiled.** There is no Android SDK or NDK in the
environment this was written in, so the Kotlin and the JNI layer are the
interface worked out in full and not a thing that has been seen to run. They are
a starting point for someone with a toolchain, not a shipped app. Treat every
line as unverified.

Before it can ship it needs, at least: the folder-picker flow and persisted
permission grant, the settings screen that writes the profile file, a
foreground-service notification for long sweeps, `cargo-ndk` in the build, and a
real device to find out how `WorkManager` actually behaves under Doze.

## iOS

**The periodic runner is not buildable on iOS, and that is not a matter of
effort.**

- `BGProcessingTask` is opportunistic. The system decides whether and when to
  run it, typically overnight on a charger, and will not run it at all for an
  app the user rarely opens. A schedule cannot be promised.
- The photo library is not a folder. Editing an asset goes through
  `PHPhotoLibrary` with a per-asset change request, and writing back a modified
  original prompts the user for each one. A silent background sweep of the
  camera roll is not a thing the platform permits, by design.

So iOS gets the **Share Extension** the root README already scoped: the file is
cleaned on its way out, at the moment it is being sent, which is the moment that
matters anyway. An in-app folder for files the app itself owns can be swept on a
`BGProcessingTask`, and that is worth having, but it must not be described to
users as "your photos are cleaned automatically", because it is not.

This asymmetry should be stated plainly in both stores' listings rather than
papered over. An honest "cleans on sharing" beats a promise the platform will
quietly break.

## Open decisions

1. **Which folders should the picker suggest?** `DCIM/Camera` is the clear case:
   a photograph is geotagged the moment it is taken, the user did not choose to
   put it there, and nothing in a camera roll is a document anyone has an
   obligation about. `Downloads` is more coverage and more chance of catching a
   file an app is mid-way through writing, which the thirty-second settle rule
   makes unlikely rather than impossible. `Documents` should not be suggested at
   all: it is where a file someone has an evidentiary obligation about would be,
   and the runner is the one part of this tool that cleans in place with nobody
   watching. The user can always grant it; the picker should not propose it.
2. **Does the runner notify on every sweep, or only when it found something?**
   Silence is pleasant and also indistinguishable from the runner being dead,
   which is a bad property for a tool whose whole job is invisible.
3. **What happens to a file the user later wants the original of?**
   `keep_original` exists and is off by default, because a dirty original next to
   a clean copy in the same gallery folder protects nobody. That default needs
   testing on real users before it ships.
