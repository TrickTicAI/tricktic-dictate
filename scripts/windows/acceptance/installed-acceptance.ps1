# MW-SEC-21: installed-app acceptance inside a disposable standard Windows account.
# Launch through as-user.ps1. Only synthetic audio; no credentials, models or keys.
# <Shared> holds candidate.exe, bridge.exe (the version on the shared stable.json
# bridge), legacy.exe (0.20.0, pre-metadata client), hardened.exe (0.20.1),
# payload-files.json ({path: sha256} of the candidate payload), update-driver.mjs
# and a writable out\ directory. Phases: fresh, legacy, previous, hardened, manual, interrupt, micdeny, micblocked, files, cleanup.
# The previous phase also needs previous.exe (the previous stable installer).
param([Parameter(Mandatory=$true)][string]$Shared, [Parameter(Mandatory=$true)][string]$Phase, [string]$OtherUserData = '',
      [string]$Bridge = '0.22.0', [string]$Version = '0.23.0')
$ErrorActionPreference = 'Stop'
$out = Join-Path $Shared 'out'
$receipt = [ordered]@{ schema = 1; kind = 'mw-sec-21-installed'; phase = $Phase; user = $env:USERNAME; started = (Get-Date).ToUniversalTime().ToString('o'); checks = [ordered]@{} }
# The Metrix Writer folder is in use until the first new-identity launch moves it.
$data = @((Join-Path $env:LOCALAPPDATA 'com.tricktic.dictate'), (Join-Path $env:LOCALAPPDATA 'MetrixWriter')) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (!$data) { $data = Join-Path $env:LOCALAPPDATA 'com.tricktic.dictate' }
$node = 'C:\Program Files\nodejs\node.exe'
function Check($name, $value) { $receipt.checks[$name] = $value }
function Entry { Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -match 'Writer|Dictate' } | Select-Object -First 1 }
function Exe { $e = Entry; if ($e) { @('tricktic-dictate.exe', 'metrix-writer-desktop.exe') | ForEach-Object { Join-Path ($e.InstallLocation.Trim('"')) $_ } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1 } }
# Process paths are not readable here, so stop every Writer process this account
# may stop. A standard account cannot stop another user's Writer.
function Quit { Get-Process tricktic-dictate,metrix-writer-desktop -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }; Start-Sleep 3 }
function Install($installer) {
  Quit
  $p = Start-Process -FilePath $installer -ArgumentList '/S' -PassThru; $null = $p.Handle
  if (!$p.WaitForExit(600000)) { throw "installer timed out: $installer" }
  Start-Sleep 3; Quit
  $e = Entry
  [ordered]@{ exit = $p.ExitCode; name = $e.DisplayName; version = $e.DisplayVersion; location = $e.InstallLocation }
}
function Uninstall {
  $e = Entry; if (!$e) { return 'not installed' }
  Quit
  $command = $e.UninstallString.Trim('"')
  $p = Start-Process -FilePath $command -ArgumentList '/S' -PassThru; $null = $p.Handle
  $null = $p.WaitForExit(300000); Start-Sleep 5
  [ordered]@{ exit = $p.ExitCode; registered_after = [bool](Entry); install_dir_after = (Test-Path -LiteralPath ($e.InstallLocation.Trim('"'))); data_retained = (Test-Path -LiteralPath $data) }
}
function Seed {
  # Same interrupted-capture fixture as scripts/desktop/smoke.ps1, in the real default root.
  New-Item -ItemType Directory -Force (Join-Path $data 'State') | Out-Null
  [IO.File]::WriteAllText((Join-Path $data 'State\onboarding.json'), '{"completed_version":1}')
  $id = [Guid]::NewGuid().ToString(); $dir = Join-Path $data "Data\native-recordings\$id"
  New-Item -ItemType Directory -Force $dir | Out-Null
  $audio = New-Object byte[] (80 + 32000)
  for ($i = 80; $i -lt $audio.Length; $i += 2) { $s = [int16](4000 * [Math]::Sin(($i - 80) / 2 * 2 * [Math]::PI * 440 / 16000)); $b = [BitConverter]::GetBytes($s); $audio[$i] = $b[0]; $audio[$i + 1] = $b[1] }
  [IO.File]::WriteAllBytes((Join-Path $dir 'audio.wav'), $audio)
  $m = @{ id = $id; created_at_ms = 1700000000000; input_language = 'mixed'; duration_ms = 0; byte_length = 0; mime_type = 'audio/wav'; capture_state = 'capturing'; chunk_count = 0; sample_rate = 16000; channels = 1 }
  [IO.File]::WriteAllText((Join-Path $dir 'recording.json'), ($m | ConvertTo-Json -Compress))
  # Synthetic successful raw transcript beside the interrupted audio fixture.
  $jobId = [Guid]::NewGuid().ToString()
  $options = @{selection=@{provider='openai';model='gpt-transcribe'};input_language='mixed';output_language=@{mode='preserve'};vocabulary=@{hints=@();corrections=@();snippets=@()};cleanup=@{mode='off'}}
  $job = @{id=$jobId;request=@{recording_id=$id;idempotency_key=[Guid]::NewGuid().ToString();options=$options};created_at_ms=1700000000001;updated_at_ms=1700000000002;status=@{state='succeeded';result=@{transcript=@{raw_text='synthetic fixture original';selection=$options.selection;execution_location='cloud';input_language='mixed';output_language=@{mode='preserve'}};cleanup=@{state='off'}}}}
  New-Item -ItemType Directory -Force (Join-Path $data 'Data\jobs') | Out-Null
  [IO.File]::WriteAllText((Join-Path $data "Data\jobs\$jobId.json"), ($job | ConvertTo-Json -Depth 20 -Compress), (New-Object Text.UTF8Encoding($false)))
  $id
}
function Saved {
  # Hash every saved recording and job file under the real data root.
  $result = [ordered]@{}
  foreach ($sub in 'Data\native-recordings', 'Data\recordings', 'Data\jobs') {
    $p = Join-Path $data $sub
    if (Test-Path $p) { Get-ChildItem -LiteralPath $p -Recurse -File | Where-Object { $_.Extension -ne '.tmp' } | ForEach-Object { $result[$_.FullName.Substring($data.Length + 1)] = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant() } }
  }
  $result
}
function Driver($steps, $expect, $extra = @()) {
  $a = @((Join-Path $Shared 'update-driver.mjs'), '--exe', (Exe), '--out', $out, '--steps', $steps) + $extra
  if ($expect) { $a += @('--expect', $expect) }
  $log = Join-Path $out ("driver-$Phase-" + [Guid]::NewGuid().ToString('N').Substring(0, 8) + '.log')
  # Start-Process joins arguments with spaces; quote each one ("Metrix Writer").
  $p = Start-Process -FilePath $node -ArgumentList ($a | ForEach-Object { '"' + $_ + '"' }) -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err" -WindowStyle Hidden
  $null = $p.Handle; $null = $p.WaitForExit(3600000)
  [ordered]@{ exit = $p.ExitCode; log = (Split-Path -Leaf $log) }
}
function Compare-Installed {
  # Installed tree versus the extracted candidate payload: hash mismatches and
  # files the installer did not ship (for example left over from older versions).
  $expected = Get-Content -Raw (Join-Path $Shared 'payload-files.json') | ConvertFrom-Json
  $location = (Entry).InstallLocation.Trim('"')
  $mismatch = @(); $count = 0
  foreach ($p in $expected.PSObject.Properties) {
    $file = Join-Path $location $p.Name
    if (!(Test-Path -LiteralPath $file)) { $mismatch += "missing $($p.Name)"; continue }
    if ($p.Name -eq 'uninstall.exe') { continue }
    $count++
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant() -ne $p.Value) { $mismatch += "hash $($p.Name)" }
  }
  $extra = @(Get-ChildItem -LiteralPath $location -Recurse -File | ForEach-Object { $_.FullName.Substring($location.Length + 1).Replace('\', '/') } | Where-Object { !$expected.PSObject.Properties[$_] })
  [ordered]@{ location = $location; verified = $count; mismatches = $mismatch; extra = $extra }
}
function Probe {
  $root = Join-Path $env:TEMP ('mws21-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
  New-Item -ItemType Directory -Force (Join-Path $root 'State') | Out-Null
  [IO.File]::WriteAllText((Join-Path $root 'State\onboarding.json'), '{"completed_version":1}')
  $p = Start-Process -FilePath (Exe) -ArgumentList @('--probe', ('"' + $root + '"')) -WindowStyle Hidden -PassThru; $null = $p.Handle
  $null = $p.WaitForExit(60000)
  $acl = Get-Acl -LiteralPath $root
  [ordered]@{ exit = $p.ExitCode; receipt = (Test-Path (Join-Path $root 'probe.json')); result = $(if (Test-Path (Join-Path $root 'probe.json')) { Get-Content -Raw (Join-Path $root 'probe.json') | ConvertFrom-Json });
    root_acl_protected = $acl.AreAccessRulesProtected; root_acl = @($acl.Access | ForEach-Object { "$($_.IdentityReference) $($_.AccessControlType) $($_.FileSystemRights)" }) }
}
function DataAcl { if (Test-Path $data) { $acl = Get-Acl -LiteralPath $data; [ordered]@{ protected = $acl.AreAccessRulesProtected; access = @($acl.Access | ForEach-Object { "$($_.IdentityReference) $($_.AccessControlType) $($_.FileSystemRights)" }) } } }

try {
  switch ($Phase) {
    'fresh' {
      # Start from nothing: remove any earlier install and its leftover files.
      Check 'uninstall_previous' (Uninstall)
      $leftover = Join-Path $env:LOCALAPPDATA 'Metrix Writer'
      if (Test-Path -LiteralPath $leftover) { Remove-Item -LiteralPath $leftover -Recurse -Force }
      Check 'registered_before' ([bool](Entry))
      Check 'install' (Install (Join-Path $Shared 'candidate.exe'))
      Check 'installed_files' (Compare-Installed)
      $location = (Entry).InstallLocation.Trim('"')
      $installAcl = Get-Acl -LiteralPath $location
      Check 'install_dir_acl' @($installAcl.Access | ForEach-Object { "$($_.IdentityReference) $($_.AccessControlType) $($_.FileSystemRights)" })
      Check 'installed_probe' (Probe)
      if ($OtherUserData) {
        $denied = [ordered]@{}
        foreach ($target in @($OtherUserData, (Split-Path $OtherUserData -Parent))) {
          try { $n = @(Get-ChildItem -LiteralPath $target -ErrorAction Stop).Count; $denied[$target] = "READABLE ($n entries)" } catch { $denied[$target] = $_.Exception.GetType().Name }
        }
        Check 'other_user_data_access' $denied
      }
      Check 'uninstall' (Uninstall)
    }
    'legacy' {
      # N-1 (pre-metadata 0.20.0) -> shared stable.json bridge -> N on the Windows feed.
      Check 'uninstall_previous' (Uninstall)
      Check 'install' (Install (Join-Path $Shared 'legacy.exe'))
      Check 'seeded_recording' (Seed)
      Check 'first_launch' (Driver 'status' $null)
      Quit
      $before = Saved; Check 'saved_before_update' $before
      Check 'update_to_bridge' (Driver 'check,download,install' $Bridge)
      Quit
      Check 'registered_after_bridge' ((Entry).DisplayVersion)
      # A shared bridge may already deliver the current Windows version.
      if ($Bridge -eq $Version) { Check 'check_current_platform_feed' (Driver 'check' $Version) }
      else { Check 'update_to_current' (Driver 'check,download,install' $Version) }
      Quit
      $after = Saved; Check 'saved_after_update' $after
      Check 'saved_preserved' ((($before.Keys | Where-Object { $after[$_] -ne $before[$_] }) | Measure-Object).Count -eq 0)
      Check 'registered_after' ((Entry).DisplayVersion)
      Check 'installed_files' (Compare-Installed)
      Check 'data_acl' (DataAcl)
    }
    'previous' {
      # Exact previous stable -> current stable through the installed updater.
      Check 'uninstall_previous' (Uninstall)
      Check 'install_previous' (Install (Join-Path $Shared 'previous.exe'))
      Check 'seeded_recording' (Seed)
      Check 'first_launch' (Driver 'status' $null)
      Quit
      $before = Saved; Check 'saved_before_update' $before
      Check 'update_to_current' (Driver 'check,download,install' $Version)
      Quit
      $after = Saved; Check 'saved_after_update' $after
      Check 'saved_preserved' ((($before.Keys | Where-Object { $after[$_] -ne $before[$_] }) | Measure-Object).Count -eq 0)
      Check 'registered_after' ((Entry).DisplayVersion)
      Check 'installed_files' (Compare-Installed)
      Check 'uninstall' (Uninstall)
    }
    'hardened' {
      Check 'uninstall_previous' (Uninstall)
      Check 'install' (Install (Join-Path $Shared 'hardened.exe'))
      $before = Saved; Check 'saved_before' $before
      Check 'cancel' (Driver 'check,cancel' $null)
      Quit
      Check 'interrupt' (Driver 'check,download,interrupt' $null @('--relaunch', '1'))
      Quit
      Check 'after_interrupt' ([ordered]@{ registered = (Entry).DisplayVersion; exe_exists = (Test-Path -LiteralPath (Exe)); probe = (Probe) })
      Check 'retry_update' (Driver 'check,download,install' $Version)
      Quit
      $after = Saved
      Check 'saved_preserved' ((($before.Keys | Where-Object { $after[$_] -ne $before[$_] }) | Measure-Object).Count -eq 0)
      Check 'registered_after' ((Entry).DisplayVersion)
      Check 'installed_files' (Compare-Installed)
    }
    'manual' {
      # Released bridge version -> this installer run by hand over it.
      Check 'uninstall_previous' (Uninstall)
      Check 'install_bridge' (Install (Join-Path $Shared 'bridge.exe'))
      $before = Saved
      Check 'install' (Install (Join-Path $Shared 'candidate.exe'))
      Check 'installed_files' (Compare-Installed)
      $after = Saved
      Check 'saved_preserved' ((($before.Keys | Where-Object { $after[$_] -ne $before[$_] }) | Measure-Object).Count -eq 0)
      Check 'probe' (Probe)
      Check 'data_acl' (DataAcl)
    }
    'interrupt' {
      # MW-SEC-26 W-14 on the candidate's own installer, as the updater runs it
      # (passive update mode), killed partway at several points. The published
      # feed cannot deliver an unpublished candidate, so this starts it directly.
      Check 'uninstall_previous' (Uninstall)
      Check 'install_bridge' (Install (Join-Path $Shared 'bridge.exe'))
      Check 'seeded_recording' (Seed)
      $before = Saved
      $runs = @()
      foreach ($delay in 1, 3, 6) {
        Quit
        $p = Start-Process -FilePath (Join-Path $Shared 'candidate.exe') -ArgumentList '/P', '/UPDATE' -PassThru; $null = $p.Handle
        Start-Sleep -Seconds $delay
        $finished = $p.HasExited
        if (!$finished) { Stop-Process -Id $p.Id -Force; $null = $p.WaitForExit(30000) }
        Start-Sleep 2; Quit
        $runs += [ordered]@{ killed_after_s = $delay; finished_before_kill = $finished; registered = (Entry).DisplayVersion;
          exe_exists = (Test-Path -LiteralPath (Exe)); probe = (Probe) }
      }
      Check 'interrupted_runs' $runs
      Check 'install' (Install (Join-Path $Shared 'candidate.exe'))
      Check 'installed_files' (Compare-Installed)
      $after = Saved
      Check 'saved_preserved' ((($before.Keys | Where-Object { $after[$_] -ne $before[$_] }) | Measure-Object).Count -eq 0)
      Check 'probe' (Probe)
    }
    'micdeny' {
      # Per-user consent, restored afterwards. MW-SEC-21 denied only desktop apps
      # (NonPackaged), which did not stop capture; MW-SEC-26 also denies the
      # user's global microphone switch, as Settings > Privacy > Microphone does.
      $store = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone'
      $keys = @($store, "$store\NonPackaged")
      $previous = [ordered]@{}
      foreach ($key in $keys) {
        # New-Item -Force would recreate an existing key and drop its subkeys.
        if (!(Test-Path -Path $key)) { New-Item -Force -Path $key | Out-Null }
        $previous[$key] = (Get-ItemProperty -Path $key -Name Value -ErrorAction SilentlyContinue).Value
        Set-ItemProperty -Path $key -Name Value -Value 'Deny'
      }
      try { Check 'denied_capture' (Driver 'capture' $null) } finally {
        foreach ($key in $keys) { if ($previous[$key]) { Set-ItemProperty -Path $key -Name Value -Value $previous[$key] } else { Remove-ItemProperty -Path $key -Name Value -ErrorAction SilentlyContinue } }
      }
      Check 'consent_restored_to' $previous
      Check 'probe_after' (Probe)
    }
    'micblocked' {
      # The operator uses Windows Settings to turn off device-wide microphone
      # access. HKCU ConsentStore edits alone need not affect desktop capture.
      $value = Get-ItemPropertyValue 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone' -Name Value
      Check 'device_microphone_access' $value
      Quit
      if ($value -ne 'Deny') { throw 'Turn off Microphone access in Windows Settings before this phase.' }
      $driver = Driver 'capture' $null
      Check 'denied_capture' $driver
      if ($driver.exit -ne 0) { throw 'Expected microphone denial was not observed; inspect driver receipt.' }
      Check 'probe_after' (Probe)
    }
    'files' { Check 'registered' ((Entry).DisplayVersion); Check 'installed_files' (Compare-Installed) }
    'cleanup' {
      # Leave the disposable account without Writer, its install folder or any data.
      Check 'uninstall' (Uninstall)
      foreach ($dir in @((Join-Path $env:LOCALAPPDATA 'Programs\TrickTic Dictate'), (Join-Path $env:LOCALAPPDATA 'Metrix Writer'), $data)) { if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force } }
      Check 'install_dir_removed' (!(Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'Metrix Writer')))
      Check 'data_removed' (!(Test-Path -LiteralPath $data))
    }
    default { throw "unknown phase $Phase" }
  }
  $receipt.completed = $true
} catch {
  $receipt.error = $_.Exception.Message
} finally {
  Quit
  $receipt.finished = (Get-Date).ToUniversalTime().ToString('o')
  $receipt | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $out "installed-$Phase.json")
}
