# NOT BUILT. See README.md in this folder.
#
# The JNI entry points are reached by name from Rust, so the shrinker cannot see
# that anything calls them. Losing them would produce an app that installs,
# starts, and does nothing on a schedule.
-keepclasseswithmembernames class dev.figura.munda.Native { native <methods>; }
-keep class dev.figura.munda.Native { *; }

# WorkManager instantiates the worker reflectively.
-keep class dev.figura.munda.SweepWorker { <init>(...); }
