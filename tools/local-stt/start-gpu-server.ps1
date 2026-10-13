# Starts a NEW Writer-owned whisper.cpp instance. Does not stop existing services.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ModelPath,
    [Parameter(Mandatory=$true)][string]$DataDirectory,
    [ValidateRange(1024,65535)][int]$Port = 8179,
    [string]$ListenAddress = '127.0.0.1',
    [ValidateSet('he','auto')][string]$Language = 'he'
)
$ErrorActionPreference = 'Stop'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$ModelPath = (Resolve-Path -LiteralPath $ModelPath).Path
if ((Get-Item -LiteralPath $ModelPath).Length -eq 0) { throw 'Model file is empty.' }
$binaryDirectory = Split-Path -Parent $Executable
if (!(Test-Path -LiteralPath (Join-Path $binaryDirectory 'ggml-vulkan.dll'))) {
    throw 'Vulkan backend DLL is missing. CPU fallback is not permitted.'
}
if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
    throw 'Selected port is occupied. Choose an isolated port.'
}
# Refuse to mix this run with another instance or overwrite existing logs.
if (Test-Path -LiteralPath $DataDirectory) { throw 'Choose a new private data directory.' }
$null = New-Item -ItemType Directory -Path $DataDirectory
$DataDirectory = (Resolve-Path -LiteralPath $DataDirectory).Path
$stdoutPath = Join-Path $DataDirectory 'server.stdout.log'
$stderrPath = Join-Path $DataDirectory 'server.stderr.log'
# Start-Process joins ArgumentList on Windows; reject quotes and quote each path.
if ($ModelPath.Contains('"') -or $DataDirectory.Contains('"') -or $ListenAddress -notmatch '^[0-9a-fA-F.:]+$') {
    throw 'Invalid path or bind address.'
}
$arguments = @('-m', ('"' + $ModelPath + '"'), '-l', $Language, '--host', $ListenAddress,
    '--port', "$Port", '-t', '8', '--tmp-dir', ('"' + $DataDirectory + '"'))
$child = Start-Process -FilePath $Executable -ArgumentList $arguments -WorkingDirectory $DataDirectory `
    -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -PassThru
try {
    $healthAddress = if ($ListenAddress -eq '0.0.0.0') { '127.0.0.1' } else { $ListenAddress }
    if ($healthAddress.Contains(':')) { $healthAddress = '[' + $healthAddress + ']' }
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    $verified = $false
    while ([DateTime]::UtcNow -lt $deadline) {
        $child.Refresh()
        if ($child.HasExited) { throw 'whisper.cpp exited during startup; inspect private logs.' }
        if ((Get-Item -LiteralPath $stderrPath).Length -gt 524288) { throw 'Startup log exceeded 512 KiB.' }
        $startup = Get-Content -LiteralPath $stderrPath -Raw
        # A DLL or GPU enumeration alone does not prove actual backend selection.
        $gpu = $startup -match 'whisper_backend_init_gpu: using Vulkan[0-9]+ backend'
        try { $health = Invoke-RestMethod -Uri "http://${healthAddress}:$Port/health" -TimeoutSec 1 } catch { $health = $null }
        if ($null -ne $health -and $health.status -eq 'ok') {
            if (!$gpu) { throw 'Model loaded without verified Vulkan use. Refusing CPU inference.' }
            $verified = $true
            break
        }
        Start-Sleep -Milliseconds 250
    }
    if (!$verified) { throw 'Timed out waiting for a loaded Vulkan model.' }
    $receipt = [ordered]@{
        process_id = $child.Id
        port = $Port
        model_path = $ModelPath
        model_sha256 = (Get-FileHash -LiteralPath $ModelPath -Algorithm SHA256).Hash.ToLowerInvariant()
        backend = 'vulkan'
        verified_at_utc = [DateTime]::UtcNow.ToString('o')
    }
    $receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $DataDirectory 'startup.json') -Encoding UTF8
    $receipt | ConvertTo-Json
} catch {
    # Only terminate the process created by this invocation.
    if (!$child.HasExited) { Stop-Process -Id $child.Id -ErrorAction SilentlyContinue }
    throw
}
