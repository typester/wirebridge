// Kotlin is set explicitly rather than left to AGP. AGP 9 has built-in Kotlin support (so `org.jetbrains.kotlin.android`
// must not be applied), and the compose compiler plugin has to carry exactly the compiler's version. KGP on the
// buildscript classpath is what the built-in Kotlin resolves against, so naming it here pins both to one number.
// beyondmap's apps/android/build.gradle.kts does the same, for a reason of its own.
buildscript {
    repositories {
        google()
        mavenCentral()
    }

    dependencies {
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.21")
    }
}

plugins {
    id("com.android.application") version "9.5.0" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.21" apply false
}
