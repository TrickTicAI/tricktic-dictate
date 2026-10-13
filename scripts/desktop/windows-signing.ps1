param(
    [Parameter(Mandatory)][ValidateSet('preflight', 'sign', 'verify')][string]$Action,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{40}$')][string]$Thumbprint,
    [Parameter(Mandatory)][string]$Publisher,
    [Parameter(Mandatory)][uri]$TimestampUrl,
    [Parameter(Mandatory)][string]$SignTool,
    [string]$Artifact
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
try {
    if ($TimestampUrl.Scheme -cne 'https' -or $TimestampUrl.UserInfo -or $TimestampUrl.Query -or $TimestampUrl.Fragment) { throw 'An HTTPS timestamp service is required.' }
    if (-not (Test-Path -LiteralPath $SignTool -PathType Leaf)) { throw 'Windows SDK SignTool is missing.' }
    $Thumbprint = $Thumbprint.ToUpperInvariant()
    if ($Action -ne 'verify') {
        $cert = Get-Item -LiteralPath "Cert:\CurrentUser\My\$Thumbprint"
        if ($cert.Subject -cne $Publisher -or -not $cert.HasPrivateKey) { throw 'The intended publisher signing key is unavailable.' }
        $now = Get-Date
        if ($cert.NotBefore -gt $now -or $cert.NotAfter -le $now) { throw 'The signing certificate is not currently valid.' }
        $eku = @($cert.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.37' } | ForEach-Object { $_.EnhancedKeyUsages } | ForEach-Object { $_.Value })
        if ('1.3.6.1.5.5.7.3.3' -notin $eku) { throw 'The certificate must explicitly allow code signing.' }
        $chain = New-Object System.Security.Cryptography.X509Certificates.X509Chain
        try {
            $chain.ChainPolicy.RevocationMode = 'Online'
            if (-not $chain.Build($cert)) { throw 'The publisher certificate chain is not trusted.' }
        } finally { $chain.Dispose() }
    }
    if ($Action -eq 'preflight') {
        @{ status = 'Valid'; publisher = $cert.Subject; thumbprint = $cert.Thumbprint } | ConvertTo-Json -Compress
        exit 0
    }
    if (-not $Artifact -or -not (Test-Path -LiteralPath $Artifact -PathType Leaf)) { throw 'Artifact is missing.' }
    if ($Action -eq 'sign') {
        # Tauri calls this again for runtime files after their signed hashes are sealed.
        # Already signed by this identity: verify without changing a single byte.
        $existing = Get-AuthenticodeSignature -LiteralPath $Artifact
        if ($null -eq $existing.SignerCertificate -or $existing.SignerCertificate.Thumbprint -cne $Thumbprint) {
            & $SignTool sign /s My /sha1 $Thumbprint /fd SHA256 /tr $TimestampUrl.AbsoluteUri /td SHA256 $Artifact *> $null
            if ($LASTEXITCODE -ne 0) { throw 'SignTool signing or timestamping failed.' }
        }
    }
    $before = (Get-FileHash -LiteralPath $Artifact -Algorithm SHA256).Hash
    # Treat warning exit code 2 as failure too: /tw warns for missing timestamps.
    & $SignTool verify /pa /all /tw $Artifact *> $null
    if ($LASTEXITCODE -ne 0) { throw 'SignTool Authenticode/timestamp verification failed.' }
    $signature = Get-AuthenticodeSignature -LiteralPath $Artifact
    if ($signature.Status -ne 'Valid' -or $signature.SignatureType -ne 'Authenticode' -or $null -eq $signature.SignerCertificate -or $null -eq $signature.TimeStamperCertificate) { throw 'A valid embedded publisher signature and trusted timestamp are required.' }
    if ($signature.SignerCertificate.Thumbprint -cne $Thumbprint -or $signature.SignerCertificate.Subject -cne $Publisher) { throw 'Unexpected publisher identity.' }
    $after = (Get-FileHash -LiteralPath $Artifact -Algorithm SHA256).Hash
    if ($before -cne $after) { throw 'Artifact changed during verification.' }
    @{
        status = 'Valid'; publisher = $signature.SignerCertificate.Subject
        thumbprint = $signature.SignerCertificate.Thumbprint
        timestampThumbprint = $signature.TimeStamperCertificate.Thumbprint
        sha256 = $after.ToLowerInvariant()
    } | ConvertTo-Json -Compress
} catch {
    # Messages above are fixed; provider exceptions may contain sensitive diagnostics.
    [Console]::Error.WriteLine('Windows Authenticode operation failed. Check certificate identity, key access, trust, timestamp and SDK prerequisites.')
    exit 1
}
