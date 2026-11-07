# The Android host

**Nothing here has been compiled.** There is no Android SDK or NDK in the
environment this was written in. These files are the interface and the
scheduling decisions worked out in full; they are a starting point for someone
with a toolchain, not a shipped app. Every line is unverified.

What the host is responsible for, and why each piece is where it is, is in
[`../README.md`](../README.md). In short: the user grants one folder through the
Storage Access Framework, `WorkManager` wakes a worker on a schedule, and the
worker walks the folder's documents through the Rust runner one at a time. The
runner holds every decision; the host holds every byte.

## Building it, once there is a toolchain

```
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
    -o app/src/main/jniLibs build --release -p fm-runner
./gradlew assembleRelease
```

The `fm-runner` crate builds as a `cdylib` for Android only; on every other
target it is an ordinary library and the JNI module is not compiled at all.

## The first things to find out on a real device

1. **How `WorkManager` actually behaves under Doze.** A six-hour period is a
   request, not a promise, and the real interval on a phone the user rarely
   opens is an empirical question.
2. **Whether a long sweep needs a foreground service.** A camera roll of
   thousands of photographs will not finish inside the window a background
   worker gets, and the alternative to a notification is being killed halfway
   every time. The runner is built to resume, but resuming forever is not
   shipping.
3. **Whether writing through a document URI preserves the entry.** Truncating
   and rewriting a document the user granted should keep its place in the
   gallery; it should be confirmed rather than assumed, because the failure mode
   is a photograph that disappears from the user's album.
