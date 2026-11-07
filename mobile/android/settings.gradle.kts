// NOT BUILT. See README.md in this folder.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
// Gradle's own wrapper is deliberately not committed: a wrapper jar is a binary
// blob in a repository whose whole argument is that you should be able to check
// what you are running. Generate it with a Gradle you trust:
//
//   gradle wrapper --gradle-version 8.11
//
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "FiguraMunda"
include(":app")
