param(
  [Parameter(Mandatory=$true)][string]$ApiExecutable,
  [Parameter(Mandatory=$true)][string]$GpuRuntimeDirectory,
  [Parameter(Mandatory=$true)][string]$FfmpegExecutable,
  [Parameter(Mandatory=$true)][string]$TinyModel,
  [Parameter(Mandatory=$true)][string]$AudioPath,
  [Parameter(Mandatory=$true)][string]$OutputDirectory,
  [ValidateSet('automatic','cpu','gpu')][string]$ExecutionBackend='gpu',
  [ValidateSet('cpu','vulkan')][string]$ExpectedBackend='vulkan',
  [string]$ModelId='whisper-tiny',
  [ValidateSet('en','he','mixed')][string]$InputLanguage='en',
  [string]$TextModelFile='',
  [ValidateSet('automatic','cpu','gpu')][string]$TextExecutionBackend='cpu'
)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Net.Http
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Choose a fresh isolated output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$models=Join-Path $OutputDirectory 'Models'
New-Item -ItemType Directory -Path $models | Out-Null
$catalog=Get-Content -Raw (Join-Path $PSScriptRoot '../../apps/api/src/desktop_settings/model-catalog.json')|ConvertFrom-Json
$spec=$catalog|Where-Object {$_.id -eq $ModelId}
if(-not $spec){throw 'Unknown test model.'}
Copy-Item -LiteralPath $TinyModel -Destination (Join-Path $models $spec.file_name)
$client=New-Object System.Net.Http.HttpClient
$client.Timeout=[TimeSpan]::FromSeconds(180)
$worker=$null
function Start-Api {
  $listener=New-Object System.Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0)
  $listener.Start(); $script:port=$listener.LocalEndpoint.Port; $listener.Stop()
  $info=New-Object System.Diagnostics.ProcessStartInfo
  $info.FileName=(Resolve-Path -LiteralPath $ApiExecutable).ProviderPath
  $info.UseShellExecute=$false; $info.CreateNoWindow=$true
  $info.RedirectStandardOutput=$true; $info.RedirectStandardError=$true
  $info.StandardOutputEncoding=[Text.Encoding]::UTF8; $info.StandardErrorEncoding=[Text.Encoding]::UTF8
  $info.EnvironmentVariables['WRITER_API_HOST']='127.0.0.1'
  $info.EnvironmentVariables['WRITER_API_PORT']=[string]$script:port
  $info.EnvironmentVariables['WRITER_DATA_DIR']=Join-Path $OutputDirectory 'Data'
  $info.EnvironmentVariables['WRITER_STATE_DIR']=Join-Path $OutputDirectory 'State'
  $info.EnvironmentVariables['WRITER_MODELS_DIR']=$models
  $info.EnvironmentVariables['WRITER_RUNTIME_DIR']=(Resolve-Path -LiteralPath $GpuRuntimeDirectory).ProviderPath
  $info.EnvironmentVariables['WRITER_LOCAL_STT_FFMPEG']=(Resolve-Path -LiteralPath $FfmpegExecutable).ProviderPath
  foreach($name in @('OPENAI_API_KEY','GEMINI_API_KEY','WRITER_LOCAL_STT_URL','WRITER_DESKTOP_IN_PROCESS')) { $info.EnvironmentVariables.Remove($name) }
  $script:started=$false;$script:stdout=$null;$script:stderr=$null
  $script:worker=New-Object System.Diagnostics.Process
  $script:worker.StartInfo=$info
  $script:started=$script:worker.Start()
  if(-not $script:started) { throw 'API did not start.' }
  $script:stdout=$script:worker.StandardOutput.ReadToEndAsync()
  $script:stderr=$script:worker.StandardError.ReadToEndAsync()
  $deadline=[DateTime]::UtcNow.AddSeconds(30)
  while([DateTime]::UtcNow -lt $deadline -and -not $script:worker.HasExited) {
    try { $h=Invoke-RestMethod -Uri "http://127.0.0.1:$script:port/api/v1/health" -TimeoutSec 2; if($h.status -eq 'ok'){return} } catch { Start-Sleep -Milliseconds 150 }
  }
  throw 'API did not become healthy.'
}
function Stop-Api([string]$name) {
  if($script:worker) {
    if($script:started -and -not $script:worker.HasExited){$script:worker.Kill();$script:worker.WaitForExit()}
    if($script:stdout){[IO.File]::WriteAllText((Join-Path $OutputDirectory "$name-stdout.log"),$script:stdout.GetAwaiter().GetResult())}
    if($script:stderr){[IO.File]::WriteAllText((Join-Path $OutputDirectory "$name-stderr.log"),$script:stderr.GetAwaiter().GetResult())}
    $script:worker.Dispose();$script:worker=$null
  }
}
function Api([string]$path,[string]$method='GET',$data=$null) {
  $uri="http://127.0.0.1:$script:port/api/v1/$path"
  $request=New-Object System.Net.Http.HttpRequestMessage((New-Object System.Net.Http.HttpMethod($method)),$uri)
  if($null -ne $data) { $request.Content=New-Object System.Net.Http.StringContent((@{api_version='1';data=$data}|ConvertTo-Json -Depth 30 -Compress),[Text.Encoding]::UTF8,'application/json') }
  try {
    $reply=$client.SendAsync($request).GetAwaiter().GetResult()
    $reply.EnsureSuccessStatusCode()|Out-Null
    $body=$reply.Content.ReadAsStringAsync().GetAwaiter().GetResult()|ConvertFrom-Json
    if($body.api_version -ne '1'){throw 'Invalid API envelope.'}
    return $body.data
  } finally {$request.Dispose()}
}
try {
  Start-Api
  Api 'setup/execution' 'PUT' $ExecutionBackend | Out-Null
  $settings=@{ui_language='en';transcription=@{selection=@{provider='local';model=$ModelId};input_language=$InputLanguage;output_language=@{mode='preserve'};vocabulary=@{hints=@();corrections=@();snippets=@()};cleanup=@{mode='off'}}}
  Api 'settings' 'PUT' $settings | Out-Null
  Api "setup/models/$ModelId/verify" 'POST' | Out-Null
  $deadline=[DateTime]::UtcNow.AddSeconds(120)
  do {$setup=Api 'setup';if($setup.runtime.worker.state -eq 'ready'){break};Start-Sleep -Milliseconds 250} while([DateTime]::UtcNow -lt $deadline)
  if($setup.runtime.worker.state -ne 'ready'){throw 'Managed model did not become ready.'}
  if($setup.runtime.worker.backend -ne $ExpectedBackend){throw 'Requested backend was not observed.'}
  $fallbackReason=$setup.runtime.execution.fallback_reason
  if($setup.models.Count -lt 37){throw 'Expanded catalog was not exposed.'}
  $recording=[Guid]::NewGuid().ToString()
  $bytes=[IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $AudioPath).ProviderPath)
  $metadata=@{id=$recording;created_at_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();mime_type='audio/wav';byte_length=$bytes.Length;duration_ms=11088;input_language=$InputLanguage;capture_state='complete'}
  $upload=New-Object System.Net.Http.MultipartFormDataContent
  $upload.Add((New-Object System.Net.Http.StringContent((@{api_version='1';data=$metadata}|ConvertTo-Json -Compress))), 'metadata')
  $upload.Add((New-Object System.Net.Http.ByteArrayContent -ArgumentList @(,$bytes)), 'audio','sample.wav')
  $reply=$client.PostAsync("http://127.0.0.1:$script:port/api/v1/recordings",$upload).GetAwaiter().GetResult()
  $reply.EnsureSuccessStatusCode()|Out-Null; $upload.Dispose()
  $settings.transcription.vocabulary.corrections=@(@{from='country';to='COUNTRY_REPLACEMENT'})
  $job=Api 'transcriptions' 'POST' @{recording_id=$recording;idempotency_key=[Guid]::NewGuid().ToString();options=$settings.transcription}
  $deadline=[DateTime]::UtcNow.AddSeconds(180)
  do {$job=Api "jobs/$($job.id)";if($job.status.state -in @('succeeded','failed')){break};Start-Sleep -Milliseconds 250} while([DateTime]::UtcNow -lt $deadline)
  if($job.status.state -ne 'succeeded'){throw 'Real transcription did not succeed.'}
  $raw=$job.status.result.transcript.raw_text
  if([string]::IsNullOrWhiteSpace($raw)){throw 'No raw transcript was retained.'}
  $modes=Api 'modes';$modes.profiles[0].name='Fixture regular';$modes=Api 'modes' 'PUT' $modes
  $request=@{operation_id=[Guid]::NewGuid().ToString();profile_id='regular';profile_revision=$modes.profiles[0].revision;context=@{application='';selected_text='';clipboard=''}}
  $attempt=Api "jobs/$($job.id)/processing" 'POST' $request
  $deadline=[DateTime]::UtcNow.AddSeconds(15)
  do {$attempts=@(Api "jobs/$($job.id)/processing");$attempt=$attempts[0];if($attempt.state.state -ne 'pending'){break};Start-Sleep -Milliseconds 100} while([DateTime]::UtcNow -lt $deadline)
  if($attempt.state.state -ne 'succeeded' -or ($raw -match 'country' -and $attempt.state.text -notmatch 'COUNTRY_REPLACEMENT')){throw 'Regular mode did not apply the saved vocabulary.'}
  $duplicate=Api "jobs/$($job.id)/processing" 'POST' $request
  if($duplicate.id -ne $attempt.id){throw 'Duplicate processing created a second operation.'}
  $textTested=$false
  if($TextModelFile) {
    $textId='qwen3-4b-instruct-2507-q4'
    $textModels=Join-Path $models 'text';New-Item -ItemType Directory -Force $textModels|Out-Null
    New-Item -ItemType HardLink -Path (Join-Path $textModels 'qwen3-4b-instruct-q4.gguf') -Target (Resolve-Path -LiteralPath $TextModelFile).ProviderPath|Out-Null
    Api "modes/local-models/$textId/verify" 'POST' | Out-Null
    $deadline=[DateTime]::UtcNow.AddSeconds(120)
    do {$library=Api 'modes/local-models';if($library.models[0].weights.state -eq 'installed'){break};Start-Sleep -Milliseconds 250} while([DateTime]::UtcNow -lt $deadline)
    if($library.models[0].weights.state -ne 'installed'){throw 'Local text weights did not verify.'}
    Api 'modes/local-models/execution' 'POST' $TextExecutionBackend | Out-Null
    $modes=Api 'modes';$super=$modes.profiles|Where-Object {$_.id -eq 'super'}
    $super.text_model=@{provider='local';model=$textId}
    $modes=Api 'modes' 'PUT' $modes;$super=$modes.profiles|Where-Object {$_.id -eq 'super'}
    $textRequest=@{operation_id=[Guid]::NewGuid().ToString();profile_id='super';profile_revision=$super.revision;context=@{application='';selected_text='';clipboard=''}}
    $textAttempt=Api "jobs/$($job.id)/processing" 'POST' $textRequest
    $deadline=[DateTime]::UtcNow.AddSeconds(930)
    do {$textAttempt=@(Api "jobs/$($job.id)/processing")|Where-Object {$_.id -eq $textRequest.operation_id};if($textAttempt.state.state -ne 'pending'){break};Start-Sleep -Milliseconds 500} while([DateTime]::UtcNow -lt $deadline)
    if($textAttempt.state.state -ne 'succeeded' -or $textAttempt.state.text -notmatch 'COUNTRY_REPLACEMENT'){throw 'Real local Super processing failed.'}
    $textLibrary=Api 'modes/local-models'
    if(-not $textLibrary.last_execution){throw 'No completed local text backend was reported.'}
    if($TextExecutionBackend -eq 'gpu' -and $textLibrary.last_execution.backend -ne 'vulkan'){throw 'Local text GPU execution was not proved.'}
    if($TextExecutionBackend -eq 'cpu' -and $textLibrary.last_execution.backend -ne 'cpu'){throw 'Local text CPU execution was not proved.'}
    Api "modes/local-models/$textId/delete" 'POST' | Out-Null
    if(Test-Path -LiteralPath (Join-Path $textModels 'qwen3-4b-instruct-q4.gguf')){throw 'Text model deletion left weights.'}
    if(-not(Test-Path -LiteralPath $TextModelFile)){throw 'Text-model source was removed.'}
    if(@(Get-ChildItem -LiteralPath $textModels -Filter '.writer-text-key-*').Count -ne 0){throw 'Transient text worker credentials were left behind.'}
    $textTested=$true
  }
  Stop-Api 'first'
  Start-Api
  $restored=Api "jobs/$($job.id)"
  if($restored.status.result.transcript.raw_text -cne $raw){throw 'Raw transcript changed across processing/restart.'}
  $attempts=@(Api "jobs/$($job.id)/processing")
  if($attempts.Count -ne $(if($textTested){2}else{1}) -or @($attempts|Where-Object {$_.state.state -ne 'succeeded'}).Count -ne 0){throw 'Processing history did not survive restart.'}
  $retained=$client.GetByteArrayAsync("http://127.0.0.1:$script:port/api/v1/recordings/$recording/audio").GetAwaiter().GetResult()
  $hash=[Security.Cryptography.SHA256]::Create()
  if([Convert]::ToBase64String($hash.ComputeHash($retained)) -cne [Convert]::ToBase64String($hash.ComputeHash($bytes))){throw 'Saved audio changed.'}
  $hash.Dispose()
  $restoredSetup=Api 'setup'
  if($restoredSetup.runtime.execution.requested -ne $ExecutionBackend){throw 'Execution choice did not persist.'}
  Api "setup/models/$ModelId/delete" 'POST' | Out-Null
  if(Test-Path -LiteralPath (Join-Path $models $spec.file_name)){throw 'Managed weights were not deleted.'}
  if(-not (Test-Path -LiteralPath $TinyModel)){throw 'Original import source was removed.'}
  $afterDelete=Api "jobs/$($job.id)"
  if($afterDelete.status.result.transcript.raw_text -cne $raw){throw 'Model deletion changed the transcript.'}
  Copy-Item -LiteralPath $TinyModel -Destination (Join-Path $models $spec.file_name)
  Api "setup/models/$ModelId/verify" 'POST' | Out-Null
  $deadline=[DateTime]::UtcNow.AddSeconds(120)
  do {$setup=Api 'setup';if($setup.runtime.worker.state -eq 'ready'){break};Start-Sleep -Milliseconds 250} while([DateTime]::UtcNow -lt $deadline)
  if($setup.runtime.worker.state -ne 'ready' -or $setup.runtime.worker.backend -ne $ExpectedBackend){throw 'Reinstalled model did not become ready on the expected backend.'}
  $historyRequest=New-Object System.Net.Http.HttpRequestMessage([System.Net.Http.HttpMethod]::Delete,"http://127.0.0.1:$script:port/api/v1/history")
  $historyReply=$client.SendAsync($historyRequest).GetAwaiter().GetResult()
  $historyReply.EnsureSuccessStatusCode()|Out-Null
  $historyRequest.Dispose();$historyReply.Dispose()
  if(Test-Path -LiteralPath (Join-Path $OutputDirectory "State/modes/$($job.id)")){throw 'History deletion left processing attempts.'}
  if((Api 'modes').profiles[0].name -ne 'Fixture regular'){throw 'History deletion removed mode profiles.'}
  @{managed_backend=$ExpectedBackend;execution=$ExecutionBackend;fallback_reason=$fallbackReason;model_deletion=$true;reinstall=$true;mode_history_deletion=$true;catalog_models=$setup.models.Count;speech_model=$ModelId;input_language=$InputLanguage;real_transcription=$true;regular_mode=$true;local_text=$textTested;text_backend=$TextExecutionBackend;idempotency=$true;restart=$true;audio_preserved=$true;raw_preserved=$true;api_sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $ApiExecutable).Hash.ToLowerInvariant()} | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $OutputDirectory 'receipt.json')
  Write-Output 'Real saved audio -> managed transcription -> Regular mode -> restart preservation passed.'
} finally {Stop-Api 'final';$client.Dispose()}
