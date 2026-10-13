param(
  [Parameter(Mandatory=$true)][string]$RuntimeDirectory,
  [Parameter(Mandatory=$true)][string]$ModelPath,
  [Parameter(Mandatory=$true)][string]$TestExecutable,
  [Parameter(Mandatory=$true)][string]$OutputDirectory,
  [ValidateSet('cpu','vulkan')][string]$Backend = 'cpu'
)
$ErrorActionPreference = 'Stop'
$runtime = (Resolve-Path -LiteralPath $RuntimeDirectory).ProviderPath
$model = (Resolve-Path -LiteralPath $ModelPath).ProviderPath
$tests = (Resolve-Path -LiteralPath $TestExecutable).ProviderPath
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $model).Hash.ToLowerInvariant() -ne '8cdb57cbb880d313736a9bc4e3d3d2485f145b5e19cf33783746e753e82641fc') { throw 'Unexpected qualification model hash.' }
if ($Backend -eq 'cpu' -and ((Test-Path -LiteralPath (Join-Path $runtime 'ggml-vulkan.dll')) -or (Test-Path -LiteralPath (Join-Path $runtime 'ggml-cuda.dll')))) { throw 'CPU qualification requires no GPU libraries.' }
$listener = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, 0)
$listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop()
$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = Join-Path $runtime 'llama-server.exe'
$start.WorkingDirectory = $runtime
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$layers = if ($Backend -eq 'cpu') { '0' } else { '999' }
# Synthetic qualification text only; never use or discover production credentials.
$start.Arguments = "--host 127.0.0.1 --port $port -m `"$model`" -c 8192 -t 8 -ngl $layers -lv 4 --parallel 1 --alias writer-qualification --api-key writer-local-qualification --no-webui"
$worker = New-Object System.Diagnostics.Process
$worker.StartInfo = $start
$started = $false
$previousPort = $env:WRITER_TEXT_QUALIFICATION_PORT
$previousOutput = $env:WRITER_TEXT_QUALIFICATION_OUTPUT
try {
  $started = $worker.Start()
  if (-not $started) { throw 'Text worker failed to start.' }
  $stdout = $worker.StandardOutput.ReadToEndAsync()
  $stderr = $worker.StandardError.ReadToEndAsync()
  $ready = $false
  $deadline = [DateTime]::UtcNow.AddSeconds(180)
  while ([DateTime]::UtcNow -lt $deadline -and -not $worker.HasExited) {
    try {
      $health = Invoke-RestMethod -Uri "http://127.0.0.1:$port/health" -TimeoutSec 2
      if ($health.status -eq 'ok') { $ready = $true; break }
    } catch { Start-Sleep -Milliseconds 250 }
  }
  if (-not $ready) { throw 'Text worker did not become healthy.' }
  $env:WRITER_TEXT_QUALIFICATION_PORT = "$port"
  $env:WRITER_TEXT_QUALIFICATION_OUTPUT = (Resolve-Path -LiteralPath $OutputDirectory).ProviderPath
  & $tests 'cleanup::modes::local_qualification::real_local_super_and_command_policies' --ignored --exact *> (Join-Path $OutputDirectory 'tests.log')
  if ($LASTEXITCODE -ne 0) { throw 'Real text-mode policy tests failed; inspect private test log.' }
} finally {
  $env:WRITER_TEXT_QUALIFICATION_PORT = $previousPort
  $env:WRITER_TEXT_QUALIFICATION_OUTPUT = $previousOutput
  if ($started -and -not $worker.HasExited) { $worker.Kill(); $worker.WaitForExit() }
  if ($stdout) { [IO.File]::WriteAllText((Join-Path $OutputDirectory 'stdout.log'), $stdout.GetAwaiter().GetResult()) }
  if ($stderr) { [IO.File]::WriteAllText((Join-Path $OutputDirectory 'stderr.log'), $stderr.GetAwaiter().GetResult()) }
  $worker.Dispose()
}
$log = [IO.File]::ReadAllText((Join-Path $OutputDirectory 'stderr.log'))
$proved = if ($Backend -eq 'cpu') { $log -match 'offloaded 0/\d+ layers to GPU' } else { $log -match 'Vulkan\d+' -and $log -match 'offloaded [1-9]\d*/\d+ layers to GPU' }
if (-not $proved) { throw 'Text runtime backend not proved.' }
@{backend=$Backend; backend_proved=$true; policy_cases=4; policies_passed=$true;
  model_sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $model).Hash.ToLowerInvariant();
  executable_sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $start.FileName).Hash.ToLowerInvariant();
  scope='Direct local runtime and production mode policies/schema; lifecycle is covered by smoke-modes-api.ps1.'
} | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $OutputDirectory 'receipt.json')
Write-Output "$Backend real Super and Command policy qualification passed."
