param(
 [Parameter(Mandatory=$true)][string]$RuntimeDirectory,
 [Parameter(Mandatory=$true)][string]$ModelPath,
 [Parameter(Mandatory=$true)][string]$OutputDirectory,
 [switch]$Cpu,
 [int]$Threads=0,
 [int]$Port=0
)
$ErrorActionPreference='Stop'
# Bounded replacement for the old unauthenticated persistent listener. Python
# owns the anonymous pipes and keeps both per-launch secrets only in memory.
if($Port -ne 0){throw 'Private workers choose their own port; omit -Port'}
if($Threads -eq 0){$Threads=if($Cpu){[Math]::Min([Environment]::ProcessorCount,16)}else{1}}
if($Threads -lt 1){throw 'Threads must be positive'}
$exe=Join-Path ([IO.Path]::GetFullPath($RuntimeDirectory)) 'whisper-server.exe'
$model=[IO.Path]::GetFullPath($ModelPath)
$output=[IO.Path]::GetFullPath($OutputDirectory)
if(-not (Test-Path $exe) -or -not (Test-Path $model)){throw 'Runtime and model must exist'}
if(Test-Path $output){throw 'Use a fresh private output directory'}
$backend=if($Cpu){'cpu'}else{'vulkan'}
$arguments=@((Join-Path $PSScriptRoot 'worker-auth-smoke.py'),'--family','whisper','--expect-backend',$backend,
 '--',$exe,'-m',[IO.Path]::GetFileName($model),'-l','he','-t',"$Threads",'-nt','-ac','768','--port','0')
if($Cpu){$arguments+='-ng'}
Push-Location ([IO.Path]::GetDirectoryName($model))
try {
 $receipt=& python @arguments
 if($LASTEXITCODE -ne 0){throw 'Private worker smoke failed'}
 New-Item -ItemType Directory -Path $output | Out-Null
 $receipt | Set-Content -Encoding UTF8 (Join-Path $output 'ready.json')
 $receipt
} finally {Pop-Location}
