param([switch]$SkipBuild, [switch]$WithoutSpeech)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    if (-not $SkipBuild) {
        cargo build --release --locked --bin suojian --bin flashnote
        if ($LASTEXITCODE -ne 0) { throw '构建失败' }
    }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $versionMatch = Select-String -LiteralPath Cargo.toml -Pattern '^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"$' | Select-Object -First 1
    if (-not $versionMatch) { throw '找不到桌面版本号。' }
    $version = $versionMatch.Matches[0].Groups[1].Value
    $packageRoot = Join-Path $projectRoot "runtime\packages\FlashNote-$version-windows-x64-$stamp"
    New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target\release\suojian.exe') -Destination (Join-Path $packageRoot '闪记.exe')
    $cli = Join-Path $projectRoot 'target\release\flashnote.exe'
    if ((& $cli --version) -ne "flashnote $version" -or $LASTEXITCODE -ne 0) { throw 'CLI 构建产物的版本与 Cargo.toml 不一致。' }
    Copy-Item -LiteralPath $cli -Destination $packageRoot
    Copy-Item -LiteralPath $cli -Destination (Join-Path $packageRoot 'suojian-cli.exe')
    $version | Set-Content -LiteralPath (Join-Path $packageRoot 'version.txt') -Encoding ascii
    foreach ($script in 'install.ps1', 'uninstall.ps1') {
        # The launcher uses Windows PowerShell 5.1, which needs a BOM for Chinese text.
        $source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot $script))
        [IO.File]::WriteAllText((Join-Path $packageRoot $script), $source, [Text.UTF8Encoding]::new($true))
    }
    '@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
pause' | Set-Content -LiteralPath (Join-Path $packageRoot '安装或更新.cmd') -Encoding ascii
    @'
闪记 FlashNote Windows 安装包

解压后双击“安装或更新.cmd”。首次安装到当前用户的 Programs\FlashNote，升级时沿用原安装目录。
升级前请从设置的“关于与更新”中退出闪记，再运行安装脚本。
数据独立保存于本机 AppData，更新只替换程序，不覆盖便签。
也可以直接运行本目录的“闪记.exe”。
命令行工具随包安装到同一目录。在 PowerShell 中运行 .\flashnote.exe --help 查看命令。
备份与迁移：设置 → 数据管理 → 导出压缩包 / 选择压缩包。
全局快捷键、开机启动：设置 → 常规。
此安装包未进行代码签名，自动下载安装更新暂未启用。
'@ | Set-Content -LiteralPath (Join-Path $packageRoot '使用说明.txt') -Encoding utf8
    if (-not $WithoutSpeech) {
        $speech = Join-Path $projectRoot 'runtime\whisper'
        if (Test-Path -LiteralPath $speech) {
            New-Item -ItemType Directory -Path (Join-Path $packageRoot 'runtime') -Force | Out-Null
            Copy-Item -LiteralPath $speech -Destination (Join-Path $packageRoot 'runtime\whisper') -Recurse
        }
    }
    $archive = "$packageRoot.zip"
    Compress-Archive -LiteralPath $packageRoot -DestinationPath $archive
    Write-Output $archive
} finally { Pop-Location }
