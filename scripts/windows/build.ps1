param([string]$ResourceDirectory = '', [switch]$Debug, [switch]$NoBundle)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).ProviderPath
$arguments = @((Join-Path $repo 'scripts\desktop\build.mjs'))
if ($ResourceDirectory) { $arguments += @('--resources', [IO.Path]::GetFullPath($ResourceDirectory)) }
if ($Debug) { $arguments += '--debug' }
if ($NoBundle) { $arguments += '--no-bundle' }
Push-Location $repo
try { & node @arguments; if ($LASTEXITCODE -ne 0) { throw 'Tauri build failed.' } }
finally { Pop-Location }
