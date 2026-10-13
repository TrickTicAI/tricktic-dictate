# MW-RELEASE-13: rehearse the Metrix Writer -> TrickTic Dictate identity update
# inside a disposable standard Windows account. Launch through as-user.ps1.
# Only synthetic data: a fake provider key, a fake sign-in, a generated tone.
# <Shared> holds old.exe (the installed stable Metrix Writer release, e.g. 0.39.0),
# candidate.exe (a com.tricktic.dictate build) and a writable out\ directory.
# Phases:
#   baseline  install old.exe silently, start it once, seed data, register the
#             old launch-at-login task, record what exists
#   update    run candidate.exe exactly as the in-app updater does
#             (/P /UPDATE /R), let it relaunch the app, then quit it
#   verify    check install identity, shortcuts, launch at login and that every
#             seeded item is present and readable under the new data folder
#   cleanup   uninstall and remove everything this account created
param([Parameter(Mandatory=$true)][string]$Shared, [Parameter(Mandatory=$true)][string]$Phase)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Security
$out = Join-Path $Shared 'out'
$receipt = [ordered]@{ schema = 1; kind = 'mw-release-13-identity'; phase = $Phase; user = $env:USERNAME; started = (Get-Date).ToUniversalTime().ToString('o'); checks = [ordered]@{} }
$legacyData = Join-Path $env:LOCALAPPDATA 'MetrixWriter'
$data = Join-Path $env:LOCALAPPDATA 'com.tricktic.dictate'
$uninstall = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
$entropy = [Text.Encoding]::ASCII.GetBytes('MetrixWriter.Credential.v1')
$key = 'sk-synthetic-mw-release-13-' + $env:USERNAME
$session = '{"version":1,"project_id":"metrix-writer","uid":"synthetic-uid","refresh_token":"synthetic-refresh","email":"synthetic@example.invalid","name":"Synthetic"}'
$term = 'TrickTicIdentityRehearsal'
function Check($name, $value) { $receipt.checks[$name] = $value }
function Quit { Get-Process tricktic-dictate,metrix-writer-desktop -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }; Start-Sleep 3 }
function Entry($name) { Get-ItemProperty (Join-Path $uninstall $name) -ErrorAction SilentlyContinue }
function Location($entry) { if ($entry) { $entry.InstallLocation.Trim('"') } }
function Protect([string]$magic, [string]$text) {
  $blob = [Security.Cryptography.ProtectedData]::Protect([Text.Encoding]::UTF8.GetBytes($text), $entropy, 'CurrentUser')
  [Text.Encoding]::ASCII.GetBytes($magic) + $blob
}
function Unprotect([string]$path, [string]$magic) {
  if (!(Test-Path -LiteralPath $path)) { return $null }
  $bytes = [IO.File]::ReadAllBytes($path); $m = [Text.Encoding]::ASCII.GetBytes($magic)
  if ($bytes.Length -le $m.Length) { return $null }
  [Text.Encoding]::UTF8.GetString([Security.Cryptography.ProtectedData]::Unprotect($bytes[$m.Length..($bytes.Length - 1)], $entropy, 'CurrentUser'))
}
function Task($name) {
  $previous = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
  $xml = schtasks.exe /Query /TN $name /XML 2>$null
  $ErrorActionPreference = $previous
  if ($LASTEXITCODE -ne 0) { return $null }
  ([xml]($xml -join "`n")).Task.Actions.Exec.Command
}
function Run($name) { (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).$name }
function Lnk($path) {
  if (!(Test-Path -LiteralPath $path)) { return $null }
  (New-Object -ComObject WScript.Shell).CreateShortcut($path).TargetPath
}
function Shortcuts {
  $result = [ordered]@{}
  foreach ($dir in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop'))) {
    foreach ($name in @('TrickTic Dictate.lnk', 'Metrix Writer.lnk')) {
      $path = Join-Path $dir $name
      if (Test-Path -LiteralPath $path) { $result[$path] = Lnk $path }
    }
  }
  $result
}
function Hashes($root) {
  $result = [ordered]@{}
  foreach ($sub in @('Data\native-recordings', 'Data\jobs', 'Data\vocabulary.json', 'State\credentials', 'State\account', 'State\onboarding.json')) {
    $p = Join-Path $root $sub
    if (Test-Path -LiteralPath $p) {
      Get-ChildItem -LiteralPath $p -Recurse -File -ErrorAction SilentlyContinue | Where-Object { $_.Extension -ne '.tmp' } | Sort-Object FullName | ForEach-Object {
        $result[$_.FullName.Substring($root.Length + 1)] = (Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant()
      }
    }
  }
  $result
}
# The same definition shell/system.rs registers, under the Metrix Writer name.
function Register-LegacyTask($exe) {
  $user = "$env:USERDOMAIN\$env:USERNAME"
  $xml = @"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>$user</UserId><Delay>PT0S</Delay></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>$user</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>
  <Actions Context="Author"><Exec><Command>$exe</Command><Arguments>--autostart</Arguments></Exec></Actions>
</Task>
"@
  $file = Join-Path $env:TEMP 'mw-release-13-task.xml'
  [IO.File]::WriteAllText($file, $xml, [Text.Encoding]::Unicode)
  # A process started through as-user.ps1 (secondary logon) may be refused by
  # Task Scheduler; then launch at login cannot be rehearsed in this account.
  $previous = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
  $result = schtasks.exe /Create /TN MetrixWriter /XML $file /F 2>&1
  $ErrorActionPreference = $previous
  Remove-Item $file
  Check 'old_task_create' ([string]$result)
}
function Launch($exe, $seconds) {
  $p = Start-Process -FilePath $exe -PassThru; $null = $p.Handle
  Start-Sleep $seconds
  Quit
}
try {
  switch ($Phase) {
    'baseline' {
      if ((Entry 'Metrix Writer') -or (Entry 'TrickTic Dictate') -or (Test-Path $legacyData) -or (Test-Path $data)) { throw 'Baseline needs an account without Writer or its data.' }
      Quit
      $p = Start-Process -FilePath (Join-Path $Shared 'old.exe') -ArgumentList '/S' -PassThru; $null = $p.Handle
      if (!$p.WaitForExit(600000)) { throw 'old installer timed out' }
      Start-Sleep 3; Quit
      $old = Entry 'Metrix Writer'
      Check 'old_registered' ([ordered]@{ name = $old.DisplayName; version = $old.DisplayVersion; location = (Location $old) })
      $oldExe = Join-Path (Location $old) 'metrix-writer-desktop.exe'
      Launch $oldExe 25
      Check 'old_data_created' (Test-Path (Join-Path $legacyData 'Data'))
      # Seeded items, written the way the old app stores them.
      New-Item -ItemType Directory -Force (Join-Path $legacyData 'State\credentials'), (Join-Path $legacyData 'State\account'), (Join-Path $legacyData 'Data\native-recordings') | Out-Null
      [IO.File]::WriteAllBytes((Join-Path $legacyData 'State\credentials\openai.cred'), (Protect "MWCRED1`n" $key))
      [IO.File]::WriteAllBytes((Join-Path $legacyData 'State\account\google-session.bin'), (Protect "MWACCT1`n" $session))
      [IO.File]::WriteAllText((Join-Path $legacyData 'State\onboarding.json'), '{"completed_version":1}')
      $id = [Guid]::NewGuid().ToString(); $dir = Join-Path $legacyData "Data\native-recordings\$id"
      New-Item -ItemType Directory -Force $dir | Out-Null
      $audio = New-Object byte[] (80 + 32000)
      for ($i = 80; $i -lt $audio.Length; $i += 2) { $s = [int16](4000 * [Math]::Sin(($i - 80) / 2 * 2 * [Math]::PI * 440 / 16000)); $b = [BitConverter]::GetBytes($s); $audio[$i] = $b[0]; $audio[$i + 1] = $b[1] }
      [IO.File]::WriteAllBytes((Join-Path $dir 'audio.wav'), $audio)
      $m = @{ id = $id; created_at_ms = 1700000000000; input_language = 'mixed'; duration_ms = 0; byte_length = 0; mime_type = 'audio/wav'; capture_state = 'capturing'; chunk_count = 0; sample_rate = 16000; channels = 1 }
      [IO.File]::WriteAllText((Join-Path $dir 'recording.json'), ($m | ConvertTo-Json -Compress))
      $vocabulary = Join-Path $legacyData 'Data\vocabulary.json'
      [IO.File]::WriteAllText($vocabulary, (@{ hints = @($term); corrections = @(); snippets = @() } | ConvertTo-Json -Compress), (New-Object Text.UTF8Encoding($false)))
      # Launch at login as 0.39 registers it: a logon task named MetrixWriter.
      Register-LegacyTask $oldExe
      Check 'old_task' (Task 'MetrixWriter')
      # Start the old app once more so it opens the seeded data, like a real install.
      Launch $oldExe 25
      Check 'shortcuts' (Shortcuts)
      Check 'hashes' (Hashes $legacyData)
      Check 'recording_id' $id
      $receipt.passed = Test-Path $oldExe
    }
    'update' {
      Quit
      # The arguments tauri-plugin-updater passes to an NSIS installer in passive mode.
      $p = Start-Process -FilePath (Join-Path $Shared 'candidate.exe') -ArgumentList '/P','/UPDATE','/R' -PassThru; $null = $p.Handle
      if (!$p.WaitForExit(600000)) { throw 'candidate installer timed out' }
      Check 'installer_exit' $p.ExitCode
      # /R relaunches the app; its first launch performs the carry-over.
      Start-Sleep 30
      Check 'running_after_update' (@(Get-Process tricktic-dictate -ErrorAction SilentlyContinue).Count)
      Quit
      $receipt.passed = $p.ExitCode -eq 0
    }
    'verify' {
      $baseline = Get-Content -Raw (Join-Path $out 'identity-baseline.json') | ConvertFrom-Json
      $new = Entry 'TrickTic Dictate'
      $newExe = Join-Path (Location $new) 'tricktic-dictate.exe'
      $oldLocation = $baseline.checks.old_registered.location
      Check 'new_registered' ([ordered]@{ name = $new.DisplayName; version = $new.DisplayVersion; publisher = $new.Publisher; location = (Location $new) })
      Check 'new_install_location_key' ((Get-ItemProperty 'HKCU:\Software\TrickTic\TrickTic Dictate' -ErrorAction SilentlyContinue).'(default)')
      Check 'old_uninstall_entry_removed' (-not (Entry 'Metrix Writer'))
      Check 'old_install_location_key_removed' (-not (Test-Path 'HKCU:\Software\metrix\Metrix Writer'))
      Check 'old_install_folder_removed' (-not (Test-Path -LiteralPath $oldLocation))
      Check 'shortcuts' (Shortcuts)
      $targets = @((Shortcuts).Values)
      Check 'shortcuts_open_new_exe' (($targets.Count -gt 0) -and -not ($targets | Where-Object { $_ -ne $newExe }))
      Check 'legacy_data_folder_gone' (-not (Test-Path -LiteralPath $legacyData))
      $log = Get-Content -Raw (Join-Path $data 'State\startup.log') -ErrorAction SilentlyContinue
      Check 'carry_over_logged' ($log -match 'identity carry-over: Moved')
      $before = $baseline.checks.hashes; $after = Hashes $data
      $missing = @($before.PSObject.Properties | Where-Object { $after[$_.Name] -ne $_.Value } | ForEach-Object { $_.Name })
      Check 'seeded_files_identical' ($missing.Count -eq 0)
      Check 'changed_or_missing' $missing
      Check 'provider_key_readable' ((Unprotect (Join-Path $data 'State\credentials\openai.cred') "MWCRED1`n") -eq $key)
      Check 'sign_in_readable' ((Unprotect (Join-Path $data 'State\account\google-session.bin') "MWACCT1`n") -eq $session)
      Check 'dictionary_term' ((Get-Content -Raw (Join-Path $data 'Data\vocabulary.json')) -match $term)
      Check 'recording_audio' (Test-Path (Join-Path $data ("Data\native-recordings\" + $baseline.checks.recording_id + '\audio.wav')))
      Check 'webview_profile' (Test-Path (Join-Path $data 'TauriWebView2'))
      Check 'task_new' (Task 'TrickTicDictate')
      Check 'task_old_removed' ((Task 'MetrixWriter') -eq $null)
      Check 'run_values' ([ordered]@{ new = (Run 'TrickTicDictate'); old = (Run 'MetrixWriter') })
      $c = $receipt.checks
      $receipt.passed = ($new -ne $null) -and $c.old_uninstall_entry_removed -and $c.old_install_location_key_removed -and $c.old_install_folder_removed `
        -and $c.shortcuts_open_new_exe -and $c.legacy_data_folder_gone -and $c.carry_over_logged -and $c.seeded_files_identical `
        -and $c.provider_key_readable -and $c.sign_in_readable -and $c.dictionary_term -and $c.recording_audio `
        -and ($c.new_install_location_key -eq (Location $new))
      # Launch at login is judged only when baseline could register the old task.
      $c.launch_at_login_rehearsed = $baseline.checks.old_task -ne $null
      if ($c.launch_at_login_rehearsed) { $receipt.passed = $receipt.passed -and ($c.task_new -eq $newExe) -and $c.task_old_removed }
    }
    'cleanup' {
      Quit
      foreach ($name in @('TrickTic Dictate', 'Metrix Writer')) {
        $e = Entry $name
        if ($e) { $p = Start-Process -FilePath $e.UninstallString.Trim('"') -ArgumentList '/S' -PassThru; $null = $p.Handle; $null = $p.WaitForExit(300000); Start-Sleep 5 }
      }
      foreach ($dir in @($data, $legacyData, (Join-Path $env:LOCALAPPDATA 'TrickTic Dictate'), (Join-Path $env:LOCALAPPDATA 'Metrix Writer'), (Join-Path $env:LOCALAPPDATA 'Programs\Metrix Writer'))) {
        if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force }
      }
      Check 'nothing_left' (-not ((Entry 'TrickTic Dictate') -or (Entry 'Metrix Writer') -or (Test-Path $data) -or (Test-Path $legacyData)))
      $receipt.passed = $receipt.checks.nothing_left
    }
    default { throw "Unknown phase $Phase" }
  }
} catch { $receipt.error = $_.Exception.Message; $receipt.passed = $false }
finally {
  $receipt.finished = (Get-Date).ToUniversalTime().ToString('o')
  $receipt | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $out "identity-$Phase.json") -Encoding UTF8
}
if (!$receipt.passed) { exit 1 }
