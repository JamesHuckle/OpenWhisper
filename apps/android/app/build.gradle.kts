plugins {
    id("com.android.application")
}

// The dictation rules the desktop app also follows: the shortest recording
// worth sending, the silence level, the prompt-echo check, the largest upload
// and the "No audio found" text. See packages/protocol/README.md.
val audioRules = groovy.json.JsonSlurper()
    .parse(rootProject.file("../../packages/protocol/audio-rules.json")) as Map<*, *>

// The transcription model the desktop app also uses by default.
val models = groovy.json.JsonSlurper()
    .parse(rootProject.file("../../packages/protocol/models.json")) as Map<*, *>

android {
    namespace = "com.openwhisper.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.openwhisper.android"
        minSdk = 28
        targetSdk = 36
        versionCode = 29
        versionName = "0.2.27"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"

        buildConfigField("double", "MIN_SPEECH_SECONDS", "${audioRules["min_speech_seconds"]}")
        buildConfigField("int", "SILENT_PCM_PEAK", "${audioRules["silent_pcm_peak"]}")
        buildConfigField(
            "double",
            "PROMPT_ECHO_MIN_SHARE",
            "${audioRules["prompt_echo_min_share"]}",
        )
        buildConfigField("long", "MAX_UPLOAD_BYTES", "${audioRules["max_upload_bytes"]}L")
        buildConfigField("String", "NO_AUDIO_MESSAGE", "\"${audioRules["no_audio_message"]}\"")
        buildConfigField(
            "String",
            "TRANSCRIPTION_MODEL",
            "\"${models["default_transcription_model"]}\"",
        )
    }

    buildFeatures {
        buildConfig = true
    }

    // The persistent release key from scripts/setup-android-signing.ps1 (or the
    // release workflow's secrets). Gradle aligns and signs the release APK with
    // it; without OPENWHISPER_ANDROID_KEYSTORE the release APK stays unsigned
    // and debug builds and tests work as before.
    signingConfigs {
        create("release") {
            val releaseKeystore = System.getenv("OPENWHISPER_ANDROID_KEYSTORE")
            if (!releaseKeystore.isNullOrBlank()) {
                storeFile = file(releaseKeystore)
                storePassword = System.getenv("OPENWHISPER_ANDROID_STORE_PASSWORD")
                keyAlias = System.getenv("OPENWHISPER_ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("OPENWHISPER_ANDROID_KEY_PASSWORD")
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            buildConfigField("boolean", "DEMO_TRANSCRIBER", "true")
        }
        release {
            buildConfigField("boolean", "DEMO_TRANSCRIBER", "false")
            signingConfig = signingConfigs.getByName("release").takeIf { it.storeFile != null }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

dependencies {
    implementation("androidx.core:core:1.17.0")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20260522")

    androidTestImplementation("androidx.test:core:1.7.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
}
