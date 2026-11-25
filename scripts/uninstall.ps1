$ErrorActionPreference = 'Stop'
$targetRoot = [IO.Path]::GetFullPath($PSScriptRoot)
$targetExe = Join-Path $targetRoot '闪记.exe'
if (-not (Test-Path -LiteralPath $targetExe)) { throw '请从安装目录运行卸载脚本。' }
if (Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $targetExe }) {
    throw '请先从设置或托盘退出闪记，再卸载。'
}
# Remove only named application files. User notes and unrelated files are preserved.
foreach ($name in @('闪记.exe', '索笺.exe', '索笺同步工具.exe', 'flashnote.exe', 'suojian-cli.exe', 'version.txt', '使用说明.txt')) {
    $item = Join-Path $targetRoot $name
    if (Test-Path -LiteralPath $item) { Remove-Item -LiteralPath $item -Force }
}
$programs = [Environment]::GetFolderPath('Programs')
$shell = New-Object -ComObject WScript.Shell
foreach ($name in @('闪记\闪记 FlashNote.lnk', '索笺.lnk', '索笺 Suojian.lnk', '索笺\索笺 Suojian.lnk')) {
    $shortcut = Join-Path $programs $name
    if ((Test-Path -LiteralPath $shortcut) -and $shell.CreateShortcut($shortcut).TargetPath -eq $targetExe) { Remove-Item -LiteralPath $shortcut }
}
Remove-Item -LiteralPath 'HKCU:\Software\Classes\AppUserModelId\NoteHarbor.Suojian' -ErrorAction SilentlyContinue
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$startup = (Get-ItemProperty -LiteralPath $runKey -Name Suojian -ErrorAction SilentlyContinue).Suojian
if ($startup -and $startup.Contains($targetExe)) { Remove-ItemProperty -LiteralPath $runKey -Name Suojian }
Remove-Item -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Suojian' -ErrorAction SilentlyContinue
Write-Output '闪记已卸载。便签数据和语音模型保留在原位置。'
