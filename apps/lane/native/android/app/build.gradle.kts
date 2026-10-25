// SPDX-License-Identifier: Apache-2.0
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}
android {
    namespace = "io.delino.lane"
    compileSdk = 35
    ndkVersion = "27.0.12077973"
    buildFeatures { buildConfig = true }
    externalNativeBuild { cmake { path = file("src/main/cpp/CMakeLists.txt"); version = "3.22.1" } }
    buildTypes {
        getByName("debug") { manifestPlaceholders["laneScheme"] = "lane-dev" }
        getByName("release") { manifestPlaceholders["laneScheme"] = "lane" }
    }
    defaultConfig {
        applicationId = "io.delino.lane"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        providers.gradleProperty("laneStoreRoot").orNull?.let { storeRoot ->
            externalNativeBuild { cmake { arguments += "-DLANE_STORE_ROOT=$storeRoot" } }
        }
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    sourceSets["main"].assets.srcDir("../../../dist/native-assets")
}
val laneAssets = tasks.register<Sync>("prepareLaneAssets") {
    from("../../../dist/main.lynx.bundle")
    from("../../../dist/LICENSE")
    from("../../../dist/NOTICE")
    from("../../licenses") { into("licenses") }
    into("../../../dist/native-assets")
    doFirst {
        check(file("../../../dist/main.lynx.bundle").isFile) {
            "Run pnpm build from apps/lane before building Android."
        }
    }
}
tasks.named("preBuild") { dependsOn(laneAssets) }
dependencies {
    implementation("androidx.work:work-runtime:2.10.1")
    implementation("com.google.firebase:firebase-messaging:24.1.1")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20250107")
    implementation("org.lynxsdk.lynx:lynx:4.1.0")
    implementation("org.lynxsdk.lynx:lynx-jssdk:4.1.0")
    implementation("org.lynxsdk.lynx:lynx-trace:4.1.0")
    implementation("org.lynxsdk.lynx:primjs:4.1.1")
}
