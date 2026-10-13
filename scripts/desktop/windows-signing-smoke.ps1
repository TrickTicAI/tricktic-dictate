# Read-only checks on the SDK binary; tampering occurs only in an owned temp copy.
param([Parameter(Mandatory)][string]$SignTool)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$verifier = Join-Path $PSScriptRoot 'windows-signing.ps1'
$shell = Join-Path $PSHOME 'powershell.exe'
$root = Join-Path ([IO.Path]::GetTempPath()) ('writer-authenticode-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $signature = Get-AuthenticodeSignature -LiteralPath $SignTool
    if ($signature.Status -ne 'Valid' -or $null -eq $signature.TimeStamperCertificate) { throw 'Smoke needs a trusted timestamped Windows SDK SignTool binary.' }
    $thumb = $signature.SignerCertificate.Thumbprint
    $publisher = $signature.SignerCertificate.Subject
    $common = @('-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $verifier,
        '-TimestampUrl', 'https://timestamp.digicert.com', '-SignTool', $SignTool)
    $proof = & $shell @common -Action verify -Thumbprint $thumb -Publisher $publisher -Artifact $SignTool
    if ($LASTEXITCODE -ne 0 -or ($proof | ConvertFrom-Json).status -ne 'Valid') { throw 'Trusted timestamped SDK verification failed.' }
    function Assert-Rejected([string[]]$Arguments) {
        $ErrorActionPreference = 'Continue'
        $null = & $shell @common @Arguments 2>$null
        if ($LASTEXITCODE -eq 0) { throw 'Invalid signing evidence was accepted.' }
    }
    Assert-Rejected @('-Action', 'verify', '-Thumbprint', $thumb, '-Publisher', 'CN=Wrong Publisher', '-Artifact', $SignTool)
    Assert-Rejected @('-Action', 'verify', '-Thumbprint', ('0' * 40), '-Publisher', $publisher, '-Artifact', $SignTool)
    $tampered = Join-Path $root 'tampered.exe'
    Copy-Item -LiteralPath $SignTool -Destination $tampered
    $stream = [IO.File]::Open($tampered, 'Open', 'ReadWrite')
    try { $stream.Position = 80; $byte = $stream.ReadByte(); $stream.Position = 80; $stream.WriteByte($byte -bxor 1) } finally { $stream.Dispose() }
    Assert-Rejected @('-Action', 'verify', '-Thumbprint', $thumb, '-Publisher', $publisher, '-Artifact', $tampered)
    $unsigned = Join-Path $root 'unsigned.exe'
    [IO.File]::WriteAllText($unsigned, 'unsigned fixture')
    Assert-Rejected @('-Action', 'verify', '-Thumbprint', $thumb, '-Publisher', $publisher, '-Artifact', $unsigned)
    Assert-Rejected @('-Action', 'preflight', '-Thumbprint', ('0' * 40), '-Publisher', 'CN=Missing')
    Write-Output 'PASS: trusted SDK signature/timestamp; rejected wrong publisher, wrong certificate, tampered PE, unsigned artifact and missing key.'
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
