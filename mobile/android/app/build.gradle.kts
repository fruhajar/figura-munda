// NOT BUILT. See README.md in this folder.
//
// The native library comes from `cargo ndk` into app/src/main/jniLibs, which is
// a separate step rather than a Gradle task on purpose: the Rust side is the
// part that is tested, and it should be possible to build and test it without
// Gradle in the way.
plugins {
    id("com.android.application")
    kotlin("android")
}

// The native library is built by cargo-ndk into app/src/main/jniLibs, which is
// a separate step on purpose: the Rust side is the part that is tested, and it
// should be possible to build and test it without Gradle in the way.
//
//   cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
//       -o app/src/main/jniLibs build --release -p fm-runner

android {
    namespace = "dev.figura.munda"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.figura.munda"
        // Scoped storage and the Storage Access Framework route this depends on
        // are the reason for not going lower.
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            // The JNI entry points are reached by name, so the shrinker has to
            // be told; see proguard-rules.pro.
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")

    packaging {
        jniLibs {
            // One .so per ABI and nothing else; the library has no assets.
            useLegacyPackaging = false
        }
    }

    // A reproducible APK needs a fixed order and no timestamps, for the same
    // reason the engine normalises ZIP member stamps in OOXML: the container is
    // a fingerprint. This is the Gradle half of the claim the desktop's
    // tools/check-reproducible.sh checks for the binaries.
    androidResources {
        noCompress += listOf("toml")
    }

    kotlinOptions { jvmTarget = "17" }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation("androidx.work:work-runtime-ktx:2.9.1")
    // FileProvider, for handing a cleaned copy to the share sheet without
    // touching the original.
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("com.google.android.material:material:1.12.0")
}
