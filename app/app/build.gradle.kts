plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Signing release (Fase 8): keystore TIDAK di-commit. Sumber:
// 1. Env CI (decoded dari secret ANDROID_KEYSTORE_B64) — dipakai Actions.
// 2. File lokal keystore/mterm-release.jks yang di-ignore git — dipakai dev.
import java.util.Base64

val ksB64 = System.getenv("ANDROID_KEYSTORE_B64")
val localKs = rootProject.file("keystore/mterm-release.jks")
val haveKs = ksB64 != null || localKs.exists()

android {
    namespace = "com.mterm.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.mterm.app"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            if (haveKs) {
                signingConfig = signingConfigs.create("release") {
                    if (ksB64 != null) {
                        val keystoreFile = layout.buildDirectory.file("release.jks").get().asFile
                        keystoreFile.writeBytes(Base64.getDecoder().decode(ksB64))
                        storeFile = keystoreFile
                    } else {
                        storeFile = localKs
                    }
                    storePassword = System.getenv("ANDROID_KEYSTORE_PASS") ?: "mterm-release-2026"
                    keyAlias = System.getenv("ANDROID_KEY_ALIAS") ?: "mterm"
                    keyPassword = System.getenv("ANDROID_KEY_PASS") ?: "mterm-release-2026"
                }
            }
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)

    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
}