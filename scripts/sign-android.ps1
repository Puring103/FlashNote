param(
    [Parameter(Mandatory)][string]$InputApk,
    [Parameter(Mandatory)][string]$OutputApk,
    [Parameter(Mandatory)][string]$KeyStore,
    [string]$SdkRoot = $env:ANDROID_HOME
)
$ErrorActionPreference = 'Stop'
if (-not $env:ANDROID_KEYSTORE_PASSWORD) { throw '缺少 ANDROID_KEYSTORE_PASSWORD。' }
$signer = Join-Path $SdkRoot 'build-tools/35.0.0/apksigner.bat'
$aligner = Join-Path $SdkRoot 'build-tools/35.0.0/zipalign.exe'
$aligned = "$OutputApk.aligned.apk"
try {
    & $aligner -P 16 -f 4 $InputApk $aligned
    if ($LASTEXITCODE -ne 0) { throw 'APK 对齐失败。' }
    & $signer sign --ks $KeyStore --ks-key-alias flashnote --ks-pass env:ANDROID_KEYSTORE_PASSWORD --key-pass env:ANDROID_KEYSTORE_PASSWORD --out $OutputApk $aligned
    if ($LASTEXITCODE -ne 0) { throw 'APK 签名失败。' }
    & $signer verify --verbose --print-certs $OutputApk
    if ($LASTEXITCODE -ne 0) { throw 'APK 签名校验失败。' }
    & $aligner -c -P 16 4 $OutputApk
    if ($LASTEXITCODE -ne 0) { throw 'APK 对齐校验失败。' }
} finally {
    if (Test-Path -LiteralPath $aligned) { Remove-Item -LiteralPath $aligned }
}
