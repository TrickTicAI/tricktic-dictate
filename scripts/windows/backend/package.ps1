param(
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [string]$BackendExecutable = '',
    [string]$WebDirectory = '',
    [string]$PackageVersion = '0.1.0'
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).ProviderPath
if (!$BackendExecutable) {
    Push-Location $repo
    try {
        $previous = $env:RUSTFLAGS
        $env:RUSTFLAGS = '-C target-feature=+crt-static'
        & cargo build --release --locked --target x86_64-pc-windows-msvc --bin metrix-writer-api
        if ($LASTEXITCODE -ne 0) { throw 'Native Rust build failed.' }
        $BackendExecutable = Join-Path $repo 'target\x86_64-pc-windows-msvc\release\metrix-writer-api.exe'
    } finally { $env:RUSTFLAGS = $previous; Pop-Location }
}
if (!$WebDirectory) {
    Push-Location $repo
    try {
        & pnpm build:web
        if ($LASTEXITCODE -ne 0) { throw 'Web build failed.' }
        $WebDirectory = Join-Path $repo 'apps\web\dist'
    } finally { Pop-Location }
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
# Staging must be empty so stale resources/credentials cannot enter a package.
if ((Test-Path $OutputDirectory) -and (Get-ChildItem -Force $OutputDirectory | Measure-Object).Count -gt 0) { throw 'Use an empty package staging directory.' }
$bytes = [IO.File]::ReadAllBytes($BackendExecutable)
if ($bytes.Length -lt 256 -or $bytes[0] -ne 77 -or $bytes[1] -ne 90) { throw 'Backend must be a native Windows PE executable.' }
$pe = [BitConverter]::ToInt32($bytes, 60)
if ($pe -lt 64 -or $pe + 6 -gt $bytes.Length -or [BitConverter]::ToUInt32($bytes, $pe) -ne 17744 -or [BitConverter]::ToUInt16($bytes, $pe + 4) -ne 34404) { throw 'Backend must be Windows x64.' }
if (!(Test-Path (Join-Path $WebDirectory 'index.html'))) { throw 'Missing built web index.' }
New-Item -ItemType Directory -Force (Join-Path $OutputDirectory 'backend'), (Join-Path $OutputDirectory 'web') | Out-Null
Copy-Item $BackendExecutable (Join-Path $OutputDirectory 'backend\metrix-writer-api.exe')
$webRoot = (Resolve-Path $WebDirectory).ProviderPath.TrimEnd('\')
foreach ($file in Get-ChildItem $webRoot -Recurse -Force) {
    if (($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Web build must not contain links.' }
    if ($file.PSIsContainer) { continue }
    $relative = $file.FullName.Substring($webRoot.Length + 1)
    if ($file.Extension -notin @('.html','.js','.css','.svg','.png','.ico','.woff','.woff2','.webp','.json','.txt')) { throw "Unexpected web resource extension: $($file.Extension)" }
    if ($file.Name.StartsWith('.') -or $relative -match '(^|[\\/])\.') { throw 'Hidden files cannot be packaged.' }
    $target = Join-Path (Join-Path $OutputDirectory 'web') $relative
    New-Item -ItemType Directory -Force (Split-Path $target) | Out-Null
    Copy-Item $file.FullName $target
}
# Enumerate only these two explicit build outputs, never a profile or repo tree.
$files = @()
foreach ($role in @('backend','web')) {
    foreach ($file in Get-ChildItem (Join-Path $OutputDirectory $role) -Recurse -File | Sort-Object FullName) {
        $files += [ordered]@{ path = $file.FullName.Substring($OutputDirectory.Length + 1).Replace('\','/'); sha256 = (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant(); bytes = $file.Length; role = $role }
    }
}
$manifest = [ordered]@{ version = 1; package_version = $PackageVersion; desktop_protocol = 1; backend_entry = 'backend/metrix-writer-api.exe'; web_root = 'web'; files = $files }
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'resources.json'), ($manifest | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding($false)))
Write-Output "Native backend/web resources staged: $OutputDirectory"
