param(
    [string]$Destination,
    [switch]$NoShortcuts,
    [switch]$RepairRegistration,
    [string]$ApplicationPath = (Join-Path $PSScriptRoot '闪记.exe')
)
$ErrorActionPreference = 'Stop'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Suojian'
if (-not $PSBoundParameters.ContainsKey('Destination')) {
    $registered = (Get-ItemProperty -LiteralPath $uninstallKey -Name InstallLocation -ErrorAction SilentlyContinue).InstallLocation
    $legacy = Join-Path $env:LOCALAPPDATA 'Programs\Suojian'
    if ($registered -and (Test-Path -LiteralPath (Join-Path $registered '索笺.exe') -PathType Leaf)) {
        $Destination = $registered
    } elseif (Test-Path -LiteralPath (Join-Path $legacy '索笺.exe') -PathType Leaf) {
        $Destination = $legacy
    } else {
        $Destination = Join-Path $env:LOCALAPPDATA 'Programs\FlashNote'
    }
}
$sourceRoot = $PSScriptRoot
$sourceExe = [IO.Path]::GetFullPath($ApplicationPath)
$targetRoot = [IO.Path]::GetFullPath($Destination)
$targetExe = Join-Path $targetRoot '索笺.exe'
if ($RepairRegistration) {
    if (-not (Test-Path -LiteralPath $targetExe)) { throw '安装目录中没有索笺.exe，请先安装程序。' }
} elseif (-not (Test-Path -LiteralPath $sourceExe)) { throw '请从解压后的安装包中运行此脚本。' }
if (-not $RepairRegistration -and $targetRoot.TrimEnd('\') -eq $sourceRoot.TrimEnd('\')) { throw '安装目录不能与安装包目录相同。' }
if (-not $RepairRegistration -and (Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $targetExe })) {
    throw '请先从闪记设置中选择“退出闪记”，然后重新安装。'
}
New-Item -ItemType Directory -Path $targetRoot -Force | Out-Null
# Replace only application files. The separate AppData database is never touched.
if (-not $RepairRegistration) {
    Copy-Item -LiteralPath $sourceExe -Destination $targetExe -Force
    foreach ($name in @('flashnote.exe', 'suojian-cli.exe', 'version.txt', '使用说明.txt')) {
        $source = Join-Path $sourceRoot $name
        if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination (Join-Path $targetRoot $name) -Force }
    }
}
$uninstallSource = Join-Path $sourceRoot 'uninstall.ps1'
$uninstallTarget = Join-Path $targetRoot 'uninstall.ps1'
if ($uninstallSource -ne $uninstallTarget) { Copy-Item -LiteralPath $uninstallSource -Destination $uninstallTarget -Force }
$wasInstalled = Test-Path -LiteralPath $uninstallKey
New-Item -Path $uninstallKey -Force | Out-Null
$uninstallCommand = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $targetRoot 'uninstall.ps1') + '"'
$versionFile = Join-Path $targetRoot 'version.txt'
$version = if (Test-Path -LiteralPath $versionFile) { (Get-Content -Raw -LiteralPath $versionFile).Trim() } else { '0.1.0' }
(@{ DisplayName = '闪记 FlashNote'; DisplayVersion = $version; Publisher = 'FlashNote'; InstallLocation = $targetRoot; DisplayIcon = $targetExe; UninstallString = $uninstallCommand }).GetEnumerator() | ForEach-Object {
    New-ItemProperty -LiteralPath $uninstallKey -Name $_.Key -Value $_.Value -PropertyType String -Force | Out-Null
}
New-ItemProperty -LiteralPath $uninstallKey -Name NoModify -Value 1 -PropertyType DWord -Force | Out-Null
New-ItemProperty -LiteralPath $uninstallKey -Name NoRepair -Value 1 -PropertyType DWord -Force | Out-Null
$speech = Join-Path $sourceRoot 'runtime'
if (-not $RepairRegistration -and (Test-Path -LiteralPath $speech)) {
    New-Item -ItemType Directory -Force -Path (Join-Path $targetRoot 'runtime') | Out-Null
    Get-ChildItem -LiteralPath $speech | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $targetRoot 'runtime') -Recurse -Force }
}
$identityKey = 'HKCU:\Software\Classes\AppUserModelId\NoteHarbor.Suojian'
New-Item -Path $identityKey -Force | Out-Null
New-ItemProperty -LiteralPath $identityKey -Name DisplayName -Value '闪记 FlashNote' -PropertyType String -Force | Out-Null
New-ItemProperty -LiteralPath $identityKey -Name IconUri -Value $targetExe -PropertyType String -Force | Out-Null
if (-not $NoShortcuts) {
    $shell = New-Object -ComObject WScript.Shell
    $programs = [Environment]::GetFolderPath('Programs')
    $menuFolder = Join-Path $programs '闪记'
    New-Item -ItemType Directory -Path $menuFolder -Force | Out-Null
    $shortcutPath = Join-Path $menuFolder '闪记 FlashNote.lnk'
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcutNeedsUpdate = -not (Test-Path -LiteralPath $shortcutPath) -or $shortcut.TargetPath -ne $targetExe -or $shortcut.WorkingDirectory -ne $targetRoot -or $shortcut.Description -ne '闪记 FlashNote 便签'
    $shortcut.TargetPath = $targetExe
    $shortcut.WorkingDirectory = $targetRoot
    $shortcut.Description = '闪记 FlashNote 便签'
    $shortcut.IconLocation = "$targetExe,0"
    if ($shortcutNeedsUpdate) { $shortcut.Save() }
    # Give the shell link a stable identity and notify the Start menu of the change.
    if (-not ('SuojianInstaller.ShellRegistration' -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
namespace SuojianInstaller {
    [StructLayout(LayoutKind.Sequential)] public struct PropertyKey { public Guid Format; public uint Id; }
    [StructLayout(LayoutKind.Explicit, Size = 24)] public struct PropVariant {
        [FieldOffset(0)] public ushort Type;
        [FieldOffset(8)] public IntPtr Text;
    }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore {
        void GetCount(out uint count);
        void GetAt(uint index, out PropertyKey key);
        void GetValue(ref PropertyKey key, out PropVariant value);
        void SetValue(ref PropertyKey key, ref PropVariant value);
        void Commit();
    }
    public static class ShellRegistration {
        [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
        static extern void SHGetPropertyStoreFromParsingName(string path, IntPtr context, uint flags, ref Guid iid, out IPropertyStore store);
        [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
        static extern void SHChangeNotify(uint change, uint flags, string first, IntPtr second);
        public static void Register(string path) {
            Guid iid = typeof(IPropertyStore).GUID;
            IPropertyStore store;
            SHGetPropertyStoreFromParsingName(path, IntPtr.Zero, 2, ref iid, out store);
            var key = new PropertyKey { Format = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), Id = 5 };
            var value = new PropVariant { Type = 31, Text = Marshal.StringToCoTaskMemUni("NoteHarbor.Suojian") };
            try { store.SetValue(ref key, ref value); store.Commit(); }
            finally { Marshal.FreeCoTaskMem(value.Text); Marshal.ReleaseComObject(store); }
            SHChangeNotify(0x00000002, 0x1005, path, IntPtr.Zero);
            SHChangeNotify(0x00001000, 0x1005, System.IO.Path.GetDirectoryName(path), IntPtr.Zero);
        }
    }
}
"@
    }
    [SuojianInstaller.ShellRegistration]::Register($shortcutPath)
    foreach ($legacyName in @('索笺.lnk', '索笺 Suojian.lnk', '索笺\索笺 Suojian.lnk')) {
        $legacyPath = Join-Path $programs $legacyName
        if ((Test-Path -LiteralPath $legacyPath) -and $shell.CreateShortcut($legacyPath).TargetPath -eq $targetExe) { Remove-Item -LiteralPath $legacyPath }
    }
    (New-Object -ComObject Shell.Application).RefreshMenu()
}
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (-not $RepairRegistration -and (-not $wasInstalled -or (Get-ItemProperty -LiteralPath $runKey -Name Suojian -ErrorAction SilentlyContinue).Suojian)) {
    New-Item -Path $runKey -Force | Out-Null
    New-ItemProperty -LiteralPath $runKey -Name Suojian -Value ('"' + $targetExe + '" --background') -PropertyType String -Force | Out-Null
}
if (-not (Get-ItemProperty -LiteralPath $uninstallKey -Name DisplayName).DisplayName) { throw '应用列表登记失败。' }
if ($RepairRegistration) { Write-Output "应用列表和开始菜单入口已修复：$targetExe" } else { Write-Output "安装完成：$targetExe" }

if (-not $RepairRegistration) {
    Start-Process -FilePath $targetExe -ArgumentList '--background' -WorkingDirectory $targetRoot -WindowStyle Hidden
}
