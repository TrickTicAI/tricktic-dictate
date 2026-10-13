<#
Copies an existing GGML weights file into Writer's per-user Models directory under the
catalog file name, then prints its SHA-256 so it can be compared with the published value.
Setup still verifies the file before loading it; this helper never starts inference.
#>
param(
    [Parameter(Mandatory=$true)][ValidateSet('ivrit-turbo', 'whisper-large-v3-turbo', 'whisper-tiny')][string]$Model,
    [Parameter(Mandatory=$true)][string]$Source,
    [string]$ModelsDirectory = (Join-Path $env:LOCALAPPDATA 'MetrixWriter\Models')
)
$ErrorActionPreference = 'Stop'
$names = @{ 'ivrit-turbo' = 'ivrit-turbo.bin'; 'whisper-large-v3-turbo' = 'ggml-large-v3-turbo.bin'; 'whisper-tiny' = 'ggml-tiny.bin' }
$expected = @{ 'ivrit-turbo' = 'c8090411113357097bfafc2b8e228ec1639fa7f5fe4ecb5d054ac0ccef8641b1'; 'whisper-large-v3-turbo' = '1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69'; 'whisper-tiny' = 'be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21' }
$Source = (Resolve-Path -LiteralPath $Source).Path
if ((Get-Item -LiteralPath $Source).Length -eq 0) { throw 'Source weights file is empty.' }
New-Item -ItemType Directory -Force $ModelsDirectory | Out-Null
$target = Join-Path $ModelsDirectory $names[$Model]
if (Test-Path -LiteralPath $target) { throw "Target already exists: $target. Remove it explicitly first; nothing was overwritten." }
$temp = "$target.import-" + [Guid]::NewGuid().ToString() + '.tmp'
Copy-Item -LiteralPath $Source -Destination $temp
$hash = (Get-FileHash -LiteralPath $temp -Algorithm SHA256).Hash.ToLowerInvariant()
if ($hash -ne $expected[$Model]) { Remove-Item -LiteralPath $temp; throw "SHA-256 $hash does not match the published $($expected[$Model]); nothing was imported." }
Move-Item -LiteralPath $temp -Destination $target
[ordered]@{ model = $Model; path = $target; bytes = (Get-Item -LiteralPath $target).Length; sha256 = $hash } | ConvertTo-Json
