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
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
        }
    }

    kotlinOptions { jvmTarget = "17" }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation("androidx.work:work-runtime-ktx:2.9.1")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("com.google.android.material:material:1.12.0")
}
