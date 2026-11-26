param(
    [switch]$SkipBuild,
    [switch]$WithoutSpeech,
    [string]$Compiler = 'ISCC.exe'
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$inno = Get-Command $Compiler -CommandType Application -ErrorAction SilentlyContinue
if (!$inno) { throw 'Inno Setup 6.4.3 or newer is required. Pass the path to ISCC.exe with -Compiler.' }

Push-Location $projectRoot
try {
    if (!$SkipBuild) {
        cargo build --release --locked --bin suojian --bin flashnote
        if ($LASTEXITCODE -ne 0) { throw '构建失败' }
    }
    $versionMatch = Select-String -LiteralPath Cargo.toml -Pattern '^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"' | Select-Object -First 1
    if (!$versionMatch) { throw '找不到桌面版本号。' }
    $version = $versionMatch.Matches[0].Groups[1].Value
    if ((& .\target\release\flashnote.exe --version) -ne "flashnote $version" -or $LASTEXITCODE -ne 0) {
        throw 'CLI 构建产物的版本与 Cargo.toml 不一致。'
    }

    $output = Join-Path $projectRoot 'runtime\packages'
    $options = @("/DVersion=$version", "/DPackageOutput=$output")
    if (!$WithoutSpeech -and (Test-Path -LiteralPath 'runtime/whisper/Release/whisper-cli.exe' -PathType Leaf)) {
        $options += '/DIncludeSpeech'
    }
    & $inno.Source @options (Join-Path $PSScriptRoot 'flashnote.iss') | Write-Host
    if ($LASTEXITCODE -ne 0) { throw 'Windows 安装包构建失败。' }
    Write-Output (Join-Path $output "FlashNote-$version-windows-x64-setup.exe")
} finally { Pop-Location }
