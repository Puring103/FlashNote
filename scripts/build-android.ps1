param(
 [ValidateSet('arm64-v8a','x86_64','all')][string]$Abi = 'arm64-v8a',
 [string]$SdkRoot = $env:ANDROID_HOME,
 [string]$Gradle,
 [switch]$DebugRust,
 [switch]$Release,
 [string]$NdkVersion = '28.0.13004108'
)
$ErrorActionPreference = 'Stop'
if ($Release -and $DebugRust) { throw '正式 APK 必须使用优化的 Rust 库。' }
$projectRoot = Split-Path -Parent $PSScriptRoot
if (-not $SdkRoot) { $SdkRoot = Join-Path $projectRoot 'runtime/android-tools/sdk' }
$SdkRoot = (Resolve-Path -LiteralPath $SdkRoot).Path
$toolchain = Join-Path $SdkRoot "ndk/$NdkVersion/toolchains/llvm/prebuilt/windows-x86_64/bin"
if (-not (Test-Path -LiteralPath (Join-Path $toolchain 'clang.exe') -PathType Leaf)) {
 throw "请通过 Android SDK Manager 安装 NDK $NdkVersion。"
}
if (-not $Gradle) { $Gradle = Join-Path $projectRoot 'apps/android/gradlew.bat' }
$targets = @(@('arm64-v8a','aarch64-linux-android','aarch64-linux-android26'),@('x86_64','x86_64-linux-android','x86_64-linux-android26'))
$selectedTargets = @($targets | Where-Object { $Abi -eq 'all' -or $_[0] -eq $Abi })
$selectedAbis = ($selectedTargets | ForEach-Object { $_[0] }) -join ','
Push-Location $projectRoot
try {
 foreach ($item in $selectedTargets) {
  $triple = $item[1]
  rustup target add $triple
  if ($LASTEXITCODE -ne 0) { throw '安装 Rust target 失败' }
  $envPrefix = $triple.ToUpper().Replace('-','_')
  $ccSuffix = $triple.Replace('-','_')
  Set-Item "Env:CARGO_TARGET_${envPrefix}_LINKER" (Join-Path $toolchain 'clang.exe')
  Set-Item "Env:CARGO_TARGET_${envPrefix}_RUSTFLAGS" "-C link-arg=--target=$($item[2]) -C link-arg=-Wl,-z,max-page-size=16384"
  Set-Item "Env:CC_$ccSuffix" (Join-Path $toolchain "$($item[2])-clang.cmd")
  Set-Item "Env:AR_$ccSuffix" (Join-Path $toolchain 'llvm-ar.exe')
  $buildArgs = @('build','--locked','-p','suojian-android','--target',$triple)
  $profile = 'debug'
  if (-not $DebugRust) { $buildArgs += '--release'; $profile = 'release' }
  cargo @buildArgs
  if ($LASTEXITCODE -ne 0) { throw "Rust Android 构建失败: $triple" }
  $out = Join-Path $projectRoot "apps/android/app/src/main/jniLibs/$($item[0])"
  New-Item -ItemType Directory -Force -Path $out | Out-Null
  Copy-Item -LiteralPath "target/$triple/$profile/libsuojian_android.so" -Destination $out
 }
 "sdk.dir=$($SdkRoot.Replace('\','/'))" | Set-Content -LiteralPath apps/android/local.properties
 $task = if ($Release) { 'assembleRelease' } else { 'assembleDebug' }
 & $Gradle -p apps/android $task --console plain "-PsuojianAbis=$selectedAbis"
 if ($LASTEXITCODE -ne 0) { throw 'Android APK 构建失败' }
 $apk = if ($Release) { 'release/app-release-unsigned.apk' } else { 'debug/app-debug.apk' }
 Write-Output (Join-Path $projectRoot "apps/android/app/build/outputs/apk/$apk")
} finally { Pop-Location }
