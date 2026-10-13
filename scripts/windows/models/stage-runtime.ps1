<#
Stages the Windows inference/conversion runtime into an existing package fragment
produced by scripts/windows/backend/package.ps1 and extends its resources.json.
Copies only the explicit whisper.cpp Vulkan server files, ffmpeg, and their notices.
Model weights are never packaged: setup imports or explicitly downloads them.
#>
param(
    [Parameter(Mandatory=$true)][string]$PackageDirectory,
    [Parameter(Mandatory=$true)][string]$WhisperDirectory,
    [Parameter(Mandatory=$true)][string]$WhisperLicense,
    [Parameter(Mandatory=$true)][string]$FfmpegDirectory,
    [string]$FfmpegLicense = ''
)
$ErrorActionPreference = 'Stop'
$PackageDirectory = [IO.Path]::GetFullPath($PackageDirectory)
$manifestPath = Join-Path $PackageDirectory 'resources.json'
if (!(Test-Path $manifestPath)) { throw 'Stage the backend/web package first (resources.json missing).' }
$manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
if ($manifest.version -ne 1) { throw 'Unsupported resource manifest version.' }
if ($manifest.files | Where-Object { $_.role -in @('runtime', 'notice') }) { throw 'Runtime resources are already staged; use a fresh package fragment.' }
if (!$FfmpegLicense) { $FfmpegLicense = Join-Path (Split-Path -Parent $FfmpegDirectory) 'LICENSE.txt' }

$whisperFiles = @('whisper-server.exe', 'whisper.dll', 'ggml.dll', 'ggml-base.dll', 'ggml-cpu.dll', 'ggml-vulkan.dll')
foreach ($name in $whisperFiles) {
    if (!(Test-Path (Join-Path $WhisperDirectory $name))) { throw "whisper.cpp runtime file missing: $name (a Vulkan build with ggml-vulkan.dll is required; CPU-only builds are refused)." }
}
$ffmpegFiles = @('ffmpeg.exe') + @(Get-ChildItem $FfmpegDirectory -File | Where-Object { $_.Name -match '^(avcodec|avformat|avutil|swresample|swscale|avfilter|avdevice)-\d+\.dll$' } | ForEach-Object { $_.Name })
if ($ffmpegFiles.Count -lt 2) { throw 'ffmpeg shared build requires ffmpeg.exe and its av*/sw* DLLs.' }
foreach ($path in @($WhisperLicense, $FfmpegLicense)) { if (!(Test-Path $path)) { throw "License/notice file missing: $path" } }

function IsPe([string]$Path) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 256 -or $bytes[0] -ne 77 -or $bytes[1] -ne 90) { return $false }
    $pe = [BitConverter]::ToInt32($bytes, 60)
    return ($pe -ge 64 -and $pe + 6 -le $bytes.Length -and [BitConverter]::ToUInt32($bytes, $pe) -eq 17744 -and [BitConverter]::ToUInt16($bytes, $pe + 4) -eq 34404)
}
$whisperTarget = Join-Path $PackageDirectory 'runtime\whisper'
$ffmpegTarget = Join-Path $PackageDirectory 'runtime\ffmpeg'
$noticeTarget = Join-Path $PackageDirectory 'notices'
New-Item -ItemType Directory -Force $whisperTarget, $ffmpegTarget, $noticeTarget | Out-Null
$entries = @()
function Stage([string]$Source, [string]$Target, [string]$Role) {
    if ($Role -eq 'runtime' -and !(IsPe $Source)) { throw "Not a Windows x64 PE file: $Source" }
    # Sources may be cloud/dedup placeholders; the staged copy must be a plain file (Resources.cs re-checks at load).
    Copy-Item -LiteralPath $Source -Destination $Target -Force
    if (((Get-Item -LiteralPath $Target).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Staged file is a reparse point: $Target" }
    $relative = $Target.Substring($PackageDirectory.Length + 1).Replace('\', '/')
    $script:entries += [ordered]@{ path = $relative; sha256 = (Get-FileHash $Target -Algorithm SHA256).Hash.ToLowerInvariant(); bytes = (Get-Item $Target).Length; role = $Role }
}
foreach ($name in $whisperFiles) { Stage (Join-Path $WhisperDirectory $name) (Join-Path $whisperTarget $name) 'runtime' }
foreach ($name in $ffmpegFiles) { Stage (Join-Path $FfmpegDirectory $name) (Join-Path $ffmpegTarget $name) 'runtime' }
Stage $WhisperLicense (Join-Path $noticeTarget 'whisper.cpp-LICENSE.txt') 'notice'
Stage $FfmpegLicense (Join-Path $noticeTarget 'ffmpeg-LICENSE.txt') 'notice'
$modelNotice = @"
Model weights are not part of this package. Transcription setup imports or explicitly
downloads them into the per-user Models directory and verifies their SHA-256:
- ivrit-turbo.bin: ivrit-ai/whisper-large-v3-turbo-ggml (Apache-2.0 model card), Hebrew only.
- ggml-large-v3-turbo.bin, ggml-tiny.bin: ggerganov/whisper.cpp (MIT; OpenAI Whisper weights MIT).
whisper.cpp runtime: MIT. ffmpeg: see ffmpeg-LICENSE.txt (GPL build).
"@
$modelNoticeSource = Join-Path ([IO.Path]::GetTempPath()) ('writer-models-notice-' + [Guid]::NewGuid().ToString('N') + '.txt')
[IO.File]::WriteAllText($modelNoticeSource, $modelNotice, (New-Object Text.UTF8Encoding($false)))
try { Stage $modelNoticeSource (Join-Path $noticeTarget 'MODELS.txt') 'notice' } finally { Remove-Item -LiteralPath $modelNoticeSource -ErrorAction SilentlyContinue }

$files = @($manifest.files | ForEach-Object { [ordered]@{ path = $_.path; sha256 = $_.sha256; bytes = $_.bytes; role = $_.role } }) + $entries
$updated = [ordered]@{ version = 1; package_version = $manifest.package_version; desktop_protocol = $manifest.desktop_protocol; backend_entry = $manifest.backend_entry; web_root = $manifest.web_root; files = $files }
[IO.File]::WriteAllText($manifestPath, ($updated | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding($false)))
Write-Output "Runtime staged: $($entries.Count) files under runtime/ and notices/ in $PackageDirectory"
