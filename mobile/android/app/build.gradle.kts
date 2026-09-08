plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

android {
    namespace = "com.termirust.mobile"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.termirust.mobile"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        manifestPlaceholders["appLabel"] = "@string/app_name"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    buildFeatures {
        compose = true
    }

    sourceSets.getByName("main") {
        java.srcDir("src/main/replication/kotlin")
        jniLibs.srcDir("src/main/replication/jniLibs")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        // Rust release artifacts are already stripped; retain their verified bytes.
        jniLibs.keepDebugSymbols += "**/libtermirust_replication_bindings.so"
        resources {
            excludes += "META-INF/versions/**/OSGI-INF/MANIFEST.MF"
        }
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2024.12.01"))
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.graphics:graphics-path:1.1.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.camera:camera-camera2:1.4.1")
    implementation("androidx.camera:camera-lifecycle:1.4.1")
    implementation("androidx.camera:camera-view:1.4.1")
    implementation("com.google.mlkit:barcode-scanning:17.3.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.8.0")
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("com.hierynomus:sshj:0.39.0")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")

    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.1")
    testRuntimeOnly("net.java.dev.jna:jna-jpms:5.17.0")

    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
    androidTestImplementation(platform("androidx.compose:compose-bom:2024.12.01"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
}

val controllerTestNativeDirectory = run {
    val os = System.getProperty("os.name").lowercase()
    val arch = System.getProperty("os.arch").lowercase()
    when {
        os.contains("mac") && (arch == "aarch64" || arch == "arm64") -> "darwin-aarch64"
        os.contains("mac") && (arch == "x86_64" || arch == "amd64") -> "darwin-x86-64"
        os.contains("linux") && (arch == "aarch64" || arch == "arm64") -> "linux-aarch64"
        os.contains("linux") && (arch == "x86_64" || arch == "amd64") -> "linux-x86-64"
        else -> error("Unsupported Controller unit-test host: $os $arch")
    }
}

tasks.withType<org.gradle.api.tasks.testing.Test>().configureEach {
    systemProperty("jna.library.path", file("src/test/native/$controllerTestNativeDirectory").absolutePath)
}
