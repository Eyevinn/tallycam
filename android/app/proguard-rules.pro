# The core's UniFFI bindings are called by JNA through reflection (structure
# fields, callback interfaces): keep them and JNA as they are.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { *; }
-dontwarn java.awt.**
-keep class se.eyevinn.tallycam.core.** { *; }
