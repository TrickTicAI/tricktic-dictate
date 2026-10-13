param(
    [Parameter(Mandatory=$true)][string]$Installer,
    [Parameter(Mandatory=$true)][string]$WeightsSource,
    [Parameter(Mandatory=$true)][string]$AudioClip,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [ValidateSet('ivrit-turbo','whisper-large-v3-turbo')][string]$Model = 'ivrit-turbo',
    [ValidateSet('he','en','mixed')][string]$Language = 'he',
    [ValidateRange(1,600)][int]$SpeechSeconds = 4,
    [ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedInstallerSha256 = '99298e0330cc3fef235c5ba8e1d80d17c493e2a2948142d65e3e605208152f01',
    [ValidatePattern('^[0-9a-f]{40}$')][string]$ReleaseCommit = '70b087f964419a0a45bf2357ee4336902a586359',
    [switch]$LongCapture,
    [switch]$ControlsOnly,
    [switch]$PackageRecovery
)
$ErrorActionPreference = 'Stop'
if ($Model -eq 'ivrit-turbo' -and $Language -ne 'he') { throw 'ivrit-turbo only supports Hebrew.' }
# Own this dedicated runner and all descendants before any application can start.
# Refuse invocation from a shared/interactive PowerShell process.
$runnerArgs = [Environment]::GetCommandLineArgs()
$fileIndex = [Array]::IndexOf($runnerArgs, '-File')
if ($fileIndex -lt 0 -or [IO.Path]::GetFullPath($runnerArgs[$fileIndex + 1]) -ne $PSCommandPath -or $runnerArgs -notcontains '-NonInteractive') { throw 'Run in a dedicated powershell -NonInteractive -File process.' }
Add-Type @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class AcceptanceOwnership {
 [StructLayout(LayoutKind.Sequential)] struct Basic { public long PerProcess, PerJob; public uint Flags; public UIntPtr Min, Max; public uint Active; public UIntPtr Affinity; public uint Priority, Scheduling; }
 [StructLayout(LayoutKind.Sequential)] struct Io { public ulong A,B,C,D,E,F; }
 [StructLayout(LayoutKind.Sequential)] struct Extended { public Basic Basic; public Io Io; public UIntPtr A,B,C,D; }
 [DllImport("kernel32.dll", CharSet=CharSet.Unicode)] static extern IntPtr CreateJobObject(IntPtr a,string n);
 [DllImport("kernel32.dll")] static extern bool SetInformationJobObject(IntPtr j,int k,ref Extended i,uint s);
 [DllImport("kernel32.dll")] static extern bool AssignProcessToJobObject(IntPtr j,IntPtr p);
 static IntPtr job;
 public static void Own() {
  job=CreateJobObject(IntPtr.Zero,null); var limits=new Extended(); limits.Basic.Flags=0x2000;
  if(job==IntPtr.Zero || !SetInformationJobObject(job,9,ref limits,(uint)Marshal.SizeOf(limits)) || !AssignProcessToJobObject(job,Process.GetCurrentProcess().Handle)) throw new Exception("Cannot own acceptance process tree");
  // Deliberately retain the handle until this dedicated process exits. Windows closes
  // it on every exit path, killing any surviving descendants, including WebView.
 }
}
'@
[AcceptanceOwnership]::Own()
if ($PSBoundParameters.ContainsKey('ExpectedInstallerSha256') -xor $PSBoundParameters.ContainsKey('ReleaseCommit')) { throw 'Supply candidate hash and source commit together.' }
$expected = $ExpectedInstallerSha256
if ((Get-FileHash $Installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Installer does not match the explicitly recorded release hash.' }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).ProviderPath
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$run = Join-Path $OutputDirectory ('acceptance-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $run | Out-Null
$install = Join-Path $run 'Programs\Writer'
$user = Join-Path $run ('Private User ' + [char]0x05D0)
$shortcuts = Join-Path $run 'Start Menu'
function Quote([string]$Value) { if ($Value.Contains('"')) { throw 'Quote in argument' }; return '"' + $Value + '"' }
function Hidden([string]$Exe, [string[]]$Arguments, [string]$Label, [int]$Seconds = 60) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Exe; $info.Arguments = $Arguments -join ' '; $info.WorkingDirectory = $run
    $info.UseShellExecute = $false; $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    $info.EnvironmentVariables['PATH'] = "$env:WINDIR\System32;$env:WINDIR"
    foreach ($key in @($info.EnvironmentVariables.Keys)) {
        if ($key -match '^(WRITER_|WEBVIEW2_|OPENAI_|GEMINI_|ANTHROPIC_|NODE_|CARGO_|RUST|WSL|VITE_)') { $info.EnvironmentVariables.Remove($key) }
    }
    $p = New-Object Diagnostics.Process; $p.StartInfo = $info
    $p.Start() | Out-Null
    $stdout = $p.StandardOutput.ReadToEndAsync(); $stderr = $p.StandardError.ReadToEndAsync()
    try {
        if (!$p.WaitForExit($Seconds * 1000)) { $p.Kill(); $p.WaitForExit(5000) | Out-Null; throw "Owned $Label process exceeded deadline." }
        if (!$stdout.Wait(5000) -or !$stderr.Wait(5000)) { throw "Owned $Label output drain exceeded deadline." }
        [IO.File]::WriteAllText((Join-Path $run "$Label.stdout.txt"), $stdout.Result)
        [IO.File]::WriteAllText((Join-Path $run "$Label.stderr.txt"), $stderr.Result)
        if ($p.ExitCode -ne 0) { throw "$Label failed; inspect private receipts in $run" }
    } finally { if (!$p.HasExited) { $p.Kill() }; $p.Dispose() }
}
Hidden $Installer @('--quiet','--install-root',(Quote $install),'--user-root',(Quote $user),'--shortcut-dir',(Quote $shortcuts),'--receipt',(Quote (Join-Path $run 'install.txt'))) 'install'
$version = (Get-Content (Join-Path $install 'current.version') -Raw).Trim()
$package = Join-Path (Join-Path $install 'versions') $version
$manifest = Get-Content (Join-Path $package 'package.json') -Raw | ConvertFrom-Json
foreach ($file in $manifest.files) {
    $path = Join-Path $package $file.path
    if ((Get-Item $path).Length -ne $file.bytes -and $null -ne $file.bytes) { throw 'Installed length mismatch' }
    if ((Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256) { throw 'Installed hash mismatch' }
}
Add-Type -Path (Join-Path $repo 'tests\windows\installed-app\ShortcutReader.cs')
$link = [WriterShortcutReader]::Read((Join-Path $shortcuts 'Metrix Writer.lnk'))
$identity = [ordered]@{ source_commit=$ReleaseCommit; integration_base_commit='48c80ea'; installer_sha256=$expected; package_version=$manifest.version; manifest_files=$manifest.files.Count; installed_executable=(Join-Path $package 'MetrixWriter.exe'); shortcut_target=$link.TargetPath; shortcut_arguments=$link.Arguments; user_root=$user; fresh_windows_account=$false; reduced_path=$true }
if ($link.TargetPath -ne (Join-Path $install 'MetrixWriter.exe')) { throw 'Shortcut target mismatch' }
$shortcutValid = $link.Arguments -eq ('--user-root ' + (Quote $user))
$identity['shortcut_arguments_valid'] = $shortcutValid
if (!$shortcutValid) { Write-Output 'FAIL artifact: shortcut does not preserve the private Unicode user root; continuing independent checks.' }
$identity | ConvertTo-Json | Set-Content (Join-Path $run 'artifact.json') -Encoding UTF8
# The normal installed launcher probes startup using its production entry point.
# It automatically exits within 45 seconds and denies all microphone permissions.
$probe = Join-Path $run 'cold-launch.txt'
Hidden (Join-Path $install 'MetrixWriter.exe') @('--user-root',(Quote $user),'--headless-probe',(Quote $probe)) 'cold-launch' 90
if ((Get-Content $probe -Raw) -notmatch 'ready:Ready:[\s\S]*stopped:Stopped') { throw 'Production cold launch failed' }
# Compile only the external reflection harness. Product binaries stay installed and unchanged.
$harness = Join-Path $run 'Harness'; New-Item -ItemType Directory $harness | Out-Null
foreach ($name in @('Microsoft.Web.WebView2.Core.dll','Microsoft.Web.WebView2.WinForms.dll','WebView2Loader.dll')) { Copy-Item (Join-Path $package $name) $harness }
$exe = Join-Path $harness 'Installed.exe'
$compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
Hidden $compiler @('/nologo','/target:exe','/platform:x64','/r:System.Windows.Forms.dll','/r:System.Drawing.dll','/r:System.Web.Extensions.dll','/r:System.Management.dll',('/r:'+(Quote (Join-Path $harness 'Microsoft.Web.WebView2.Core.dll'))),('/r:'+(Quote (Join-Path $harness 'Microsoft.Web.WebView2.WinForms.dll'))),('/out:'+(Quote $exe)),(Quote (Join-Path $repo 'tests\windows\installed-app\Installed.cs'))) 'compile'
Copy-Item (Join-Path $package 'MetrixWriter.exe.config') ($exe + '.config')
Copy-Item (Join-Path $repo 'tests\windows\installed-app\inject.js') (Join-Path $harness 'inject.js')
Copy-Item $AudioClip (Join-Path $harness 'speech.audio')
$mode = 'short'; if ($LongCapture) { $mode = 'long' }
if ($ControlsOnly) { if ($LongCapture) { throw 'Use separate control and long runtime runs.' }; $mode = 'controls' }
$sessionFailed = $false
try { Hidden $exe @((Quote $package),(Quote $user),(Quote (Join-Path $harness 'inject.js')),(Quote (Join-Path $harness 'speech.audio')),(Quote $WeightsSource),(Quote (Join-Path $run 'session.json')),$mode,$Model,$Language,$SpeechSeconds) 'session' 1200 }
catch {
    $sessionReceipt = Join-Path $run 'session.json'
    if (!(Test-Path $sessionReceipt)) { throw }
    $result = Get-Content $sessionReceipt -Raw | ConvertFrom-Json
    if ($result.status -ne 'failed' -or $result.cleanup_failed) { throw }
    $sessionFailed = $true; Write-Output "FAIL session checks; inspect session.json and private logs in $run"
}
foreach ($file in $manifest.files) { if ((Get-FileHash (Join-Path $package $file.path) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256) { throw 'Harness changed installed files' } }
if ($PackageRecovery) {
    # The existing runner adds its own private/version directories. Avoid making
    # this composition itself exceed the installer's observed MAX_PATH boundary.
    $recovery = Join-Path $env:LOCALAPPDATA ('Temp\mw22-package-' + [Guid]::NewGuid().ToString('N').Substring(0,8))
    $identity['package_recovery_output'] = $recovery
    $identity | ConvertTo-Json | Set-Content (Join-Path $run 'artifact.json') -Encoding UTF8
    & (Join-Path $repo 'scripts\windows\package\test.ps1') -Installer $Installer -OutputDirectory $recovery -ExpectedVersion $manifest.version
}
Write-Output "Completed scoped installed checks. Private receipts: $run"
if (!$shortcutValid) { throw 'Artifact acceptance failed: shortcut arguments mismatch. See artifact.json.' }
if ($sessionFailed) { throw 'Installed session acceptance failed. See session.json and private logs.' }
