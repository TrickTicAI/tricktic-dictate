param([Parameter(Mandatory=$true)][string]$Installer, [Parameter(Mandatory=$true)][string]$OutputDirectory, [string]$ExpectedVersion = '0.2.0')
$ErrorActionPreference = 'Stop'
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$run = Join-Path $OutputDirectory ('private-' + [Guid]::NewGuid().ToString('N'))
$install = Join-Path $run 'Programs\Writer'
$user = Join-Path $run ('User ' + [char]0x05D0)
$shortcuts = Join-Path $run 'Start Menu'
New-Item -ItemType Directory -Force $run | Out-Null
function Quote([string]$Text) { return '"' + $Text + '"' }
function StartHidden([string]$Exe, [string[]]$Arguments) {
    $oldPath = $env:PATH
    try {
        $env:PATH = "$env:WINDIR\System32;$env:WINDIR"
        $p = Start-Process -FilePath $Exe -ArgumentList $Arguments -WorkingDirectory $run -WindowStyle Hidden -PassThru
        $p.Handle | Out-Null
        return $p
    } finally { $env:PATH = $oldPath }
}
function Finish([Diagnostics.Process]$Process, [int]$Expected = 0) {
    if (!$Process.WaitForExit(60000)) { $Process.Kill(); throw 'Owned test process timed out.' }
    $Process.WaitForExit()
    if ($Process.ExitCode -ne $Expected) { throw "Unexpected exit $($Process.ExitCode), expected $Expected." }
}
function Install([string]$Label, [int]$Expected = 0) {
    $receipt = Join-Path $run "$Label.txt"
    $p = StartHidden $Installer @('--quiet','--install-root',(Quote $install),'--user-root',(Quote $user),'--shortcut-dir',(Quote $shortcuts),'--receipt',(Quote $receipt))
    Finish $p $Expected
    if (!(Test-Path $receipt)) { throw 'Installer receipt missing.' }
}
function Ready([string]$Receipt) {
    $deadline = [DateTime]::UtcNow.AddSeconds(40)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path $Receipt) {
            $value = Get-Content $Receipt -Raw
            if ($value.StartsWith('ready:Ready:')) { return }
            throw "Cold launch failed: $value"
        }
        Start-Sleep -Milliseconds 100
    }
    throw 'Cold launch readiness deadline expired.'
}
function Probe([string]$Label) {
    $receipt = Join-Path $run "$Label.txt"
    $p = StartHidden (Join-Path $install 'MetrixWriter.exe') @('--user-root',(Quote $user),'--headless-probe',(Quote $receipt))
    try { Ready $receipt }
    catch { [IO.File]::WriteAllText(($receipt + '.quit'), 'quit'); if (!$p.WaitForExit(60000)) { $p.Kill() }; throw }
    return @{ Process=$p; Receipt=$receipt }
}
function StopProbe($Probe) {
    [IO.File]::WriteAllText(($Probe.Receipt + '.quit'), 'quit')
    Finish $Probe.Process
    if ((Get-Content $Probe.Receipt -Raw) -notmatch 'stopped:Stopped') { throw 'Shutdown did not stop the owned backend.' }
}
Install 'install'
$first = Get-Content (Join-Path $install 'current.version') -Raw
$version = Join-Path (Join-Path $install 'versions') $first
$manifest = Get-Content (Join-Path $version 'package.json') -Raw | ConvertFrom-Json
if ($manifest.version -ne $ExpectedVersion) { throw 'Package version mismatch.' }
foreach ($entry in $manifest.files) {
    $path = Join-Path $version $entry.path
    if ((Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.sha256) { throw 'Installed hash mismatch.' }
}
$link = Join-Path $shortcuts 'Metrix Writer.lnk'
if (!(Test-Path $link)) { throw 'Launch entry missing.' }
if (!('WriterShortcutReader' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot '..\..\..\tests\windows\installed-app\ShortcutReader.cs') }
$entry = [WriterShortcutReader]::Read($link)
if ($entry.TargetPath -ne (Join-Path $install 'MetrixWriter.exe') -or $entry.Arguments -ne ('--user-root ' + (Quote $user))) { throw 'Shortcut target/arguments mismatch.' }
$probe = Probe 'cold-launch'
try {
    $second = Join-Path $run 'second.txt'
    Finish (StartHidden (Join-Path $install 'MetrixWriter.exe') @('--user-root',(Quote $user),'--headless-probe',(Quote $second)))
    if ((Get-Content $second -Raw) -ne 'second-instance') { throw 'Second instance did not hand off.' }
    Install 'update-while-running' 1
    if ((Get-Content (Join-Path $install 'current.version') -Raw) -ne $first) { throw 'Running app update changed pointer.' }
} finally { StopProbe $probe }
# Opaque fixtures prove install never rewrites saved profile/data/settings/models.
foreach ($path in @('Data\retention.bin','State\preferences-fixture.json','Models\fixture.bin','WebView2\legacy-fixture.bin')) {
    $to = Join-Path $user $path
    New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
    [IO.File]::WriteAllText($to, 'retained-' + $path)
}
$before = @{}
Get-ChildItem $user -Recurse -File | Where-Object Name -match 'fixture|retention' | ForEach-Object { $before[$_.FullName] = (Get-FileHash $_.FullName -Algorithm SHA256).Hash }
# Simulate an upgrade interrupted before pointer publication.
$partial = Join-Path $install 'versions\0.2.0-00000000000000000000000000000000'
New-Item -ItemType Directory $partial | Out-Null
[IO.File]::WriteAllText((Join-Path $partial 'partial.tmp'), 'incomplete')
$probe = Probe 'interrupted-update'; StopProbe $probe
Install 'update'
$next = Get-Content (Join-Path $install 'current.version') -Raw
if ($next -eq $first -or (Get-Content (Join-Path $install 'previous.version') -Raw) -ne $first) { throw 'Atomic version/rollback pointer failed.' }
foreach ($path in $before.Keys) { if ((Get-FileHash $path -Algorithm SHA256).Hash -ne $before[$path]) { throw 'Update changed retained data.' } }
$probe = Probe 'relaunch'; StopProbe $probe
# A broken active package fails closed; repair installs a fresh version and keeps data.
$active = Join-Path (Join-Path $install 'versions') $next
Move-Item (Join-Path $active 'backend\metrix-writer-api.exe') (Join-Path $run 'owned-backend.exe')
Finish (StartHidden (Join-Path $install 'MetrixWriter.exe') @('--user-root',(Quote $user),'--headless-probe',(Quote (Join-Path $run 'missing-backend.txt')))) 1
Install 'repair'
$probe = Probe 'repaired'; StopProbe $probe
foreach ($path in $before.Keys) { if ((Get-FileHash $path -Algorithm SHA256).Hash -ne $before[$path]) { throw 'Repair changed retained data.' } }
Write-Output "PASS installed package: manifest/version, stable shortcut, cold launch, second instance, owned shutdown, busy update refusal, interrupted upgrade, relaunch, missing backend and repair; opaque data preserved. Private evidence: $run"
