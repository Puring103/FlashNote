plugins {
 id("com.android.application")
 id("org.jetbrains.kotlin.android")
 id("org.jetbrains.kotlin.plugin.compose")
}

val supportedAbis = listOf("arm64-v8a", "x86_64")
val rustLibraries = layout.projectDirectory.dir("src/main/jniLibs")
val selectedAbis = providers.gradleProperty("suojianAbis").orNull?.split(',')
    ?: supportedAbis.filter {
        rustLibraries.file("$it/libsuojian_android.so").asFile.isFile
    }.ifEmpty { listOf("arm64-v8a") }
check(selectedAbis.all { it in supportedAbis }) { "Unsupported ABI: $selectedAbis" }

android {
 namespace = "com.noteharbor.flashnote"
 compileSdk = 35
 defaultConfig {
  applicationId = "com.noteharbor.suojian"
  minSdk = 26
  targetSdk = 35
  versionCode = 1201
  versionName = "0.12.1"
  ndk {
   abiFilters += selectedAbis
  }
 }
 buildFeatures { compose = true; buildConfig = true }
 compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
 kotlinOptions { jvmTarget = "17" }
 buildTypes { release { isMinifyEnabled = false } }
}
dependencies {
 testImplementation("junit:junit:4.13.2")
 implementation("androidx.core:core-ktx:1.16.0")
 implementation(platform("androidx.compose:compose-bom:2025.04.01"))
 implementation("androidx.activity:activity-compose:1.10.1")
 implementation("androidx.compose.material3:material3")
 implementation("androidx.compose.foundation:foundation")
 implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.0")
 implementation("androidx.lifecycle:lifecycle-viewmodel-ktx:2.9.0")
}

val verifyRustLibrary by tasks.registering {
 doLast {
  for (abi in selectedAbis) {
   check(rustLibraries.file("$abi/libsuojian_android.so").asFile.isFile) {
    "Missing shared Rust library for $abi. Run scripts/build-android.ps1 from the repository root first."
   }
  }
 }
}
tasks.named("preBuild") { dependsOn(verifyRustLibrary) }
