param(
  [Parameter(Mandatory=$true)][string]$RuntimeDirectory,
  [Parameter(Mandatory=$true)][string]$ModelPath,
  [Parameter(Mandatory=$true)][string]$AudioPath,
  [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http
$runtime = (Resolve-Path -LiteralPath $RuntimeDirectory).Path
$model = (Resolve-Path -LiteralPath $ModelPath).Path
$audio = (Resolve-Path -LiteralPath $AudioPath).Path
if (Test-Path -LiteralPath (Join-Path $runtime 'ggml-vulkan.dll')) { throw 'CPU independence test requires a package without Vulkan libraries.' }
if (Test-Path -LiteralPath (Join-Path $runtime 'ggml-cuda.dll')) { throw 'CPU independence test requires a package without CUDA libraries.' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$listener = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = $listener.LocalEndpoint.Port
$listener.Stop()
$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = Join-Path $runtime 'whisper-server.exe'
$start.WorkingDirectory = $runtime
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.StandardOutputEncoding = [Text.Encoding]::UTF8
$start.StandardErrorEncoding = [Text.Encoding]::UTF8
$start.Arguments = "--host 127.0.0.1 --port $port --no-gpu -t 4 -m `"$model`""
$worker = New-Object System.Diagnostics.Process
$worker.StartInfo = $start
$client = New-Object System.Net.Http.HttpClient
$client.Timeout = [TimeSpan]::FromSeconds(120)
$body = $null
$started = $false
try {
  $started = $worker.Start()
  if (-not $started) { throw 'Worker failed to start.' }
  $stdout = $worker.StandardOutput.ReadToEndAsync()
  $stderr = $worker.StandardError.ReadToEndAsync()
  $ready = $false
  $deadline = [DateTime]::UtcNow.AddSeconds(90)
  while ([DateTime]::UtcNow -lt $deadline -and -not $worker.HasExited) {
    try {
      $health = Invoke-RestMethod -Uri "http://127.0.0.1:$port/health" -TimeoutSec 2
      if ($health.status -eq 'ok') { $ready = $true; break }
    } catch { Start-Sleep -Milliseconds 250 }
  }
  if (-not $ready) { throw 'Worker did not become healthy.' }
  $body = New-Object System.Net.Http.MultipartFormDataContent
  $file = New-Object System.Net.Http.ByteArrayContent -ArgumentList @(,[IO.File]::ReadAllBytes($audio))
  $body.Add($file, 'file', 'sample.wav')
  $body.Add((New-Object System.Net.Http.StringContent('en')), 'language')
  $body.Add((New-Object System.Net.Http.StringContent('json')), 'response_format')
  $reply = $client.PostAsync("http://127.0.0.1:$port/inference", $body).GetAwaiter().GetResult()
  $reply.EnsureSuccessStatusCode() | Out-Null
  $text = $reply.Content.ReadAsStringAsync().GetAwaiter().GetResult()
  $result = $text | ConvertFrom-Json
  if ([string]::IsNullOrWhiteSpace($result.text)) { throw 'Worker did not return a transcript.' }
  [IO.File]::WriteAllText((Join-Path $OutputDirectory 'response.json'), $text)
  $receipt = @{ backend='cpu'; healthy=$true; transcribed=$true; model_sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $model).Hash.ToLowerInvariant(); gpu_libraries_present=$false }
} finally {
  if ($body) { $body.Dispose() }
  $client.Dispose()
  if ($started -and -not $worker.HasExited) { $worker.Kill(); $worker.WaitForExit() }
  if ($stdout) { [IO.File]::WriteAllText((Join-Path $OutputDirectory 'stdout.log'), $stdout.GetAwaiter().GetResult()) }
  if ($stderr) { [IO.File]::WriteAllText((Join-Path $OutputDirectory 'stderr.log'), $stderr.GetAwaiter().GetResult()) }
  $worker.Dispose()
}
$log = [IO.File]::ReadAllText((Join-Path $OutputDirectory 'stderr.log'))
if ($log -notmatch 'use gpu\s*=\s*0' -or $log -match 'using Vulkan') { throw 'Worker did not prove CPU execution.' }
$receipt.backend_proved=$true
$receipt | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $OutputDirectory 'receipt.json')
Write-Output 'CPU-only server health and real HTTP transcription passed.'
