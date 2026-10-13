param([Parameter(Mandatory=$true)][string]$Executable, [string]$OutputDirectory = '', [string]$ResourceDirectory = '')
$ErrorActionPreference = 'Stop'
if (!$OutputDirectory) { $OutputDirectory = Join-Path $env:TEMP ('writer-tauri-probe-' + [Guid]::NewGuid().ToString('N')) }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
# Only synthetic audio, in a new isolated root. No credentials or production data.
$root = Join-Path $OutputDirectory ('נתונים-' + [Guid]::NewGuid().ToString('N'))
# This smoke asserts the workspace/history screen. A fresh profile otherwise
# correctly opens onboarding, whose behavior has a separate browser suite.
New-Item -ItemType Directory -Force (Join-Path $root 'State') | Out-Null
[IO.File]::WriteAllText((Join-Path $root 'State/onboarding.json'), '{"completed_version":1}', (New-Object Text.UTF8Encoding($false)))
$id = [Guid]::NewGuid().ToString()
$recording = Join-Path $root "Data\native-recordings\$id"
New-Item -ItemType Directory -Force $recording | Out-Null
# Stale header + complete PCM tail simulate a process interrupted between writes.
$audio = New-Object byte[] (80 + 32000)
for ($i=80; $i -lt $audio.Length; $i+=2) { $sample=[int16](4000 * [Math]::Sin(($i-80)/2*2*[Math]::PI*440/16000)); $b=[BitConverter]::GetBytes($sample); $audio[$i]=$b[0]; $audio[$i+1]=$b[1] }
[IO.File]::WriteAllBytes((Join-Path $recording 'audio.wav'), $audio)
$metadata = @{id=$id;created_at_ms=1700000000000;input_language='mixed';duration_ms=0;byte_length=0;mime_type='audio/wav';capture_state='capturing';chunk_count=0;sample_rate=16000;channels=1}
[IO.File]::WriteAllText((Join-Path $recording 'recording.json'), ($metadata | ConvertTo-Json -Compress), (New-Object Text.UTF8Encoding($false)))
$app = Join-Path $OutputDirectory 'tricktic-dictate.exe'
Copy-Item $Executable $app
if ($ResourceDirectory) {
    foreach ($name in @('runtime', 'notices', 'runtime-manifest.json')) { if (Test-Path (Join-Path $ResourceDirectory $name)) { Copy-Item (Join-Path $ResourceDirectory $name) (Join-Path $OutputDirectory $name) -Recurse -Force } }
}
$jobId = [Guid]::NewGuid().ToString()
$options = @{selection=@{provider='openai';model='gpt-transcribe'};input_language='mixed';output_language=@{mode='preserve'};vocabulary=@{hints=@();corrections=@();snippets=@()};cleanup=@{mode='off'}}
$job = @{id=$jobId;request=@{recording_id=$id;idempotency_key=[Guid]::NewGuid().ToString();options=$options};created_at_ms=1700000000001;updated_at_ms=1700000000002;status=@{state='succeeded';result=@{transcript=@{raw_text='synthetic fixture original';selection=$options.selection;execution_location='cloud';input_language='mixed';output_language=@{mode='preserve'}};cleanup=@{state='off'}}}}
New-Item -ItemType Directory -Force (Join-Path $root 'Data\jobs') | Out-Null
[IO.File]::WriteAllText((Join-Path $root "Data\jobs\$jobId.json"), ($job | ConvertTo-Json -Depth 20 -Compress), (New-Object Text.UTF8Encoding($false)))
foreach ($pass in @('native-journal', 'backend-history')) {
    if ($pass -eq 'backend-history') { Move-Item $recording (Join-Path $OutputDirectory 'retained-journal-backup') }
    $receipt = Join-Path $root 'probe.json'
    if (Test-Path $receipt) { Move-Item $receipt (Join-Path $OutputDirectory 'native-journal.json') }
    $process = Start-Process -FilePath $app -ArgumentList @('--probe', ('"' + $root + '"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $OutputDirectory "$pass.stdout.txt") -RedirectStandardError (Join-Path $OutputDirectory "$pass.stderr.txt")
    if (!$process.WaitForExit(55000)) { Stop-Process -Id $process.Id; throw 'Hidden Writer probe exceeded its deadline.' }
    if (!(Test-Path $receipt)) { throw "No native probe receipt; inspect $OutputDirectory" }
    $result = Get-Content -Raw $receipt | ConvertFrom-Json
    Write-Output $pass
    $result | ConvertTo-Json
    if (@($result.PSObject.Properties | Where-Object { $_.Value -ne $true }).Count -gt 0) { throw 'Native probe failed.' }
}
Write-Output "Native probe passed: $OutputDirectory"
