plugins {
    id("com.android.application")
}

android {
    namespace = "com.openwhisper.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.openwhisper.android"
        minSdk = 28
        targetSdk = 36
        versionCode = 5
        versionName = "0.2.3"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    buildFeatures {
        buildConfig = true
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            buildConfigField("boolean", "DEMO_TRANSCRIBER", "true")
        }
        release {
            buildConfigField("boolean", "DEMO_TRANSCRIBER", "false")
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
