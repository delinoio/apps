// SPDX-License-Identifier: Apache-2.0
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}
android {
    namespace = "io.delino.lane"
    compileSdk = 35
    defaultConfig {
        applicationId = "io.delino.lane"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
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
    into("../../../dist/native-assets")
    doFirst {
        check(file("../../../dist/main.lynx.bundle").isFile) {
            "Run pnpm build from apps/lane before building Android."
        }
    }
}
tasks.named("preBuild") { dependsOn(laneAssets) }
dependencies {
    implementation("org.lynxsdk.lynx:lynx:4.1.0")
    implementation("org.lynxsdk.lynx:lynx-jssdk:4.1.0")
    implementation("org.lynxsdk.lynx:lynx-trace:4.1.0")
    implementation("org.lynxsdk.lynx:primjs:4.1.1")
}
