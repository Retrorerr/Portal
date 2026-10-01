# Release builds run R8 (minifyEnabled). Rust reaches Portal's Java/Kotlin
# classes only through JNI (FindClass / GetStaticMethodID / GetFieldID by
# name), which R8 cannot see, so without these rules it strips or renames
# them: SoftKeyboardBridge disappears and ComposeOverlay loses the methods the
# native side calls. Keep all of Portal's own code intact; libraries still
# shrink normally.
-keep class app.polarbear.** { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
