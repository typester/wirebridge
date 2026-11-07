import javax.inject.Inject
import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.internal.os.OperatingSystem
import org.gradle.process.ExecOperations

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// apps/android -> the repository root, where the Cargo workspace lives.
val repoRoot: File = rootProject.projectDir.parentFile.parentFile

val nativeCrate = "wirebridge-core"
val abi = "arm64-v8a"
val hostLibName = if (OperatingSystem.current().isMacOsX) "libwirebridge_core.dylib" else "libwirebridge_core.so"

// No keys go into the APK: the app makes its own key pair, and each client's, the first time it needs them (README,
// "How it works").

android {
    namespace = "dev.typester.wirebridge"
    compileSdk = 37

    // Named so that AGP can find llvm-strip; without it the unstripped Rust library goes into the APK as it is.
    ndkVersion = "29.0.14206865"

    defaultConfig {
        applicationId = "dev.typester.wirebridge"
        minSdk = 33
        // Not 37: from API 37 on, Android 17's local network protection blocks LAN traffic until the app holds
        // ACCESS_LOCAL_NETWORK, which is a runtime prompt. Below 37, INTERNET grants it implicitly, and LAN traffic is
        // all the Mac's datagrams are.
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"

        ndk { abiFilters += abi }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    // cargo-ndk copies every .so in the target directory, and boringtun declares a cdylib of its own, so a
    // libboringtun-<hash>.so turns up beside the core. Nothing loads it — boringtun is linked into
    // libwirebridge_core.so as an rlib (its NEEDED list is liblog, libdl, libm and libc) — so it is dead weight.
    // Found by listing the APK's entries; beyondmap's docs/android.md describes the same trap.
    packaging {
        jniLibs.excludes += "**/libboringtun*.so"
    }
}

// --------------------------------------------------------------- the core
//
// The same wiring as beyondmap's apps/android/app/build.gradle.kts, which explains each choice at length: one task per
// cargo step, each with a declared output directory (AGP 9 refuses a generated source directory it cannot attribute to
// a task) and the crate sources declared as inputs (or Gradle keeps yesterday's bindings). Nothing generated is
// tracked.

abstract class CargoTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations

    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @get:Internal abstract val root: DirectoryProperty

    @get:Input abstract val arguments: ListProperty<String>

    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    @TaskAction
    fun run() {
        exec.exec {
            workingDir(root.get().asFile)
            commandLine(arguments.get())
        }
    }
}

fun CargoTask.crateSources() {
    sources.from(repoRoot.resolve("crates"), repoRoot.resolve("Cargo.toml"), repoRoot.resolve("Cargo.lock"))
}

/**
 * Cross-compiles the core into the jniLibs the APK ships. Always `--release`, even for a debug APK: the point of this
 * app is to measure throughput, and WireGuard's cryptography at opt-level 0 would measure the compiler instead.
 */
val cargoNdk = tasks.register<CargoTask>("cargoNdk") {
    description = "Cross-compiles $nativeCrate for $abi"
    root.set(repoRoot)
    crateSources()
    outputDir.set(layout.buildDirectory.dir("generated/jniLibs"))
    arguments.set(
        provider {
            listOf(
                "cargo", "ndk", "-t", abi,
                "-o", outputDir.get().asFile.absolutePath,
                "build", "--release", "-p", nativeCrate,
            )
        },
    )
}

/** The host build, which uniffi-bindgen reads the interface metadata from. */
val cargoHost = tasks.register<CargoTask>("cargoHost") {
    description = "Builds $nativeCrate for the host"
    root.set(repoRoot)
    crateSources()
    outputDir.set(layout.buildDirectory.dir("generated/host"))
    arguments.set(provider { listOf("cargo", "build", "-p", nativeCrate) })
    doLast {
        copy {
            from(repoRoot.resolve("target/debug/$hostLibName"))
            into(outputDir.get().asFile)
        }
    }
}

val uniffiBindings = tasks.register<CargoTask>("uniffiBindings") {
    description = "Generates the Kotlin bindings for $nativeCrate"
    dependsOn(cargoHost)
    root.set(repoRoot)
    crateSources()
    outputDir.set(layout.buildDirectory.dir("generated/uniffi"))
    arguments.set(
        provider {
            listOf(
                "cargo", "run", "-q", "-p", nativeCrate, "--features", "bindgen",
                "--bin", "uniffi-bindgen", "--",
                "generate",
                // Switches off the generated checksum check; crates/core/bindgen.toml says why.
                "-c", repoRoot.resolve("crates/core/bindgen.toml").absolutePath,
                "--library",
                layout.buildDirectory.file("generated/host/$hostLibName").get().asFile.absolutePath,
                "--language", "kotlin",
                "--no-format",
                "--out-dir", outputDir.get().asFile.absolutePath,
            )
        },
    )
}

androidComponents.onVariants { variant ->
    variant.sources.kotlin?.addGeneratedSourceDirectory(uniffiBindings, CargoTask::outputDir)
    variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoNdk, CargoTask::outputDir)
}

// --------------------------------------------------------------- deps

dependencies {
    // What UniFFI's Kotlin bindings call the .so through.
    implementation("net.java.dev.jna:jna:5.19.1@aar")

    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.navigation:navigation-compose:2.10.2")
    // QR codes for the Wi-Fi and for each client's config: ZXing's encoder alone; Compose draws the modules.
    implementation("com.google.zxing:core:3.5.4")
    implementation("androidx.core:core-ktx:1.19.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.11.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.0")
}
