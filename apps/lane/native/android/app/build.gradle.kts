// SPDX-License-Identifier: Apache-2.0
import java.net.URI

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
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        val apiBase = providers.gradleProperty("laneApiBaseUrl").getOrElse("https://lane.api.delino.io")
        val apiUri = URI(apiBase)
        require(apiUri.scheme == "https" && apiUri.host != null && apiUri.userInfo == null &&
            apiUri.query == null && apiUri.fragment == null && apiUri.path.isEmpty()) {
            "laneApiBaseUrl must be an HTTPS origin"
        }
        val oauthClient = providers.gradleProperty("laneOAuthClientId").getOrElse("io.delino.lane.android")
        require(oauthClient.matches(Regex("[a-zA-Z0-9._-]+")))
        buildConfigField("String", "LANE_API_BASE_URL", "\"$apiBase\"")
        buildConfigField("String", "LANE_OAUTH_CLIENT_ID", "\"$oauthClient\"")
        // Optional public Firebase configuration. No Google-services file or account is
        // required for standalone Lane; provide all four values in local Gradle properties.
        val firebase = mapOf("laneFcmApplicationId" to "google_app_id", "laneFcmProjectId" to "project_id",
            "laneFcmApiKey" to "google_api_key", "laneFcmSenderId" to "gcm_defaultSenderId")
        val configured = firebase.mapValues { providers.gradleProperty(it.key).orNull }
        require(configured.values.all { it == null } || configured.values.all { !it.isNullOrBlank() }) {
            "Provide all Lane FCM configuration properties together"
        }
        configured.forEach { (property, value) -> if (value != null) resValue("string", firebase.getValue(property), value) }

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
val laneStoreRoot = file(providers.gradleProperty("laneStoreRoot").getOrElse("../../store"))
val laneAssets = tasks.register<Sync>("prepareLaneAssets") {
    from("../../../dist/main.lynx.bundle")
    from("../../../dist/LICENSE")
    from("../../../dist/NOTICE")
    from("../../licenses") { into("licenses") }
    val storeNotices = listOf("sqlite/LICENSE", "ada/LICENSE-APACHE", "ada/LICENSE-MIT",
        "json/LICENSE.MIT", "picosha2/LICENSE", "utf8proc/LICENSE.md")
    storeNotices.forEach { notice ->
        from(laneStoreRoot.resolve("vendor/$notice")) { into("licenses/store/${notice.substringBeforeLast('/')}") }
    }
    into("../../../dist/native-assets")
    doFirst {
        storeNotices.forEach { notice -> check(laneStoreRoot.resolve("vendor/$notice").isFile) {
            "The shared Lane store and its dependency notices must be present before packaging Android."
        } }
        check(file("../../../dist/main.lynx.bundle").isFile) {
            "Run pnpm build from apps/lane before building Android."
        }
    }
}
tasks.named("preBuild") { dependsOn(laneAssets) }
dependencies {
    implementation("androidx.work:work-runtime:2.10.1")
    implementation("com.google.firebase:firebase-messaging:24.1.1")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20250107")
    implementation("org.lynxsdk.lynx:lynx:4.1.0")
    implementation("org.lynxsdk.lynx:lynx-jssdk:4.1.0")
    implementation("org.lynxsdk.lynx:lynx-trace:4.1.0")
    implementation("org.lynxsdk.lynx:primjs:4.1.1")
}
