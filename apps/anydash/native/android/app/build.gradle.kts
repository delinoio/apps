// SPDX-License-Identifier: Apache-2.0
import java.net.URI

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

/** Public HTTPS origins only; credentials never enter the build. */
fun origin(name: String, fallback: String): String {
    val value = providers.gradleProperty(name).orNull ?: System.getenv(name.toEnvironmentName()) ?: fallback
    if (value.isEmpty()) return value
    val uri = URI(value)
    require(uri.scheme == "https" && uri.host != null && uri.userInfo == null && uri.query == null &&
        uri.fragment == null && (uri.path.isEmpty() || uri.path == "/")) { "$name must be an HTTPS origin" }
    return value.trimEnd('/')
}
fun String.toEnvironmentName() = "PUBLIC_" + replace(Regex("([a-z])([A-Z])"), "$1_$2").uppercase()

android {
    namespace = "io.delino.anydash"
    compileSdk = 36
    buildToolsVersion = "36.0.0"
    buildFeatures { buildConfig = true }
    defaultConfig {
        applicationId = "io.delino.anydash"
        minSdk = 29
        targetSdk = 36
        // The unified release passes its run number and release version.
        versionCode = providers.gradleProperty("anydashVersionCode").orNull?.toInt() ?: 1
        versionName = providers.gradleProperty("anydashVersionName").orNull ?: "0.1.0"
        val appId = providers.gradleProperty("anydashNativeAppId").orNull
            ?: System.getenv("PUBLIC_ANYDASH_NATIVE_APP_ID") ?: ""
        require(appId.matches(Regex("[A-Za-z0-9_-]*"))) { "anydashNativeAppId must be a Logto application ID" }
        buildConfigField("String", "ANYDASH_API_URL", "\"${origin("anydashApiUrl", "https://anydash.api.delino.io")}\"")
        buildConfigField("String", "ANYDASH_RESOURCE", "\"${origin("anydashResource", "https://anydash.api.delino.io")}\"")
        buildConfigField("String", "DELCORE_RESOURCE", "\"${origin("delcoreResource", "https://delcore.api.delino.io")}\"")
        buildConfigField("String", "LOGTO_ENDPOINT", "\"${origin("logtoEndpoint", "")}\"")
        buildConfigField("String", "LOGTO_APP_ID", "\"$appId\"")
    }
    // Release signing exists only when the protected release environment
    // supplies the upload key; local release builds stay unsigned.
    val keystore = System.getenv("ANYDASH_ANDROID_KEYSTORE_PATH")
    if (!keystore.isNullOrEmpty()) {
        signingConfigs {
            create("upload") {
                storeFile = file(keystore)
                storePassword = System.getenv("ANYDASH_ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("ANYDASH_ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("ANYDASH_ANDROID_KEY_PASSWORD")
            }
        }
        buildTypes { getByName("release") { signingConfig = signingConfigs.getByName("upload") } }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    sourceSets["main"].assets.srcDir("../../../dist/native-assets")
    testOptions { unitTests.isReturnDefaultValues = true }
}

val anydashAssets = tasks.register<Sync>("prepareAnyDashAssets") {
    from("../../../dist/main.lynx.bundle")
    from("../../../dist/LICENSE")
    from("../../../dist/NOTICE")
    from("../../licenses") { into("licenses") }
    from("../../../dist/licenses") { into("licenses/bundle") }
    into("../../../dist/native-assets")
    doFirst {
        check(file("../../../dist/main.lynx.bundle").isFile) {
            "Run pnpm build from apps/anydash before building Android."
        }
    }
}
tasks.named("preBuild") { dependsOn(anydashAssets) }
// JVM tests decode the shared fixtures produced by the TypeScript bindings.
tasks.withType<Test>().configureEach {
    systemProperty("anydash.fixtures", file("../../widgets/fixtures").absolutePath)
}

dependencies {
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20250107")
    implementation("org.lynxsdk.lynx:lynx:4.1.0")
    implementation("org.lynxsdk.lynx:xelement-input:4.1.0")
    implementation("org.lynxsdk.lynx:lynx-jssdk:4.1.0")
    implementation("org.lynxsdk.lynx:lynx-trace:4.1.0")
    implementation("org.lynxsdk.lynx:primjs:4.1.1")
}
