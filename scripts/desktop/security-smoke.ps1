param([Parameter(Mandatory=$true)][string]$Executable,[Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
$output=[IO.Path]::GetFullPath($OutputDirectory)
if(Test-Path -LiteralPath $output){throw 'Use a new private output directory.'}
New-Item -ItemType Directory -Path $output | Out-Null
$app=Join-Path $output 'tricktic-dictate.exe'
Copy-Item -LiteralPath $Executable -Destination $app
$root=Join-Path $output 'profile'
$process=Start-Process -FilePath $app -ArgumentList @('--security-probe',('"'+$root+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $output 'stdout.txt') -RedirectStandardError (Join-Path $output 'stderr.txt')
$null=$process.Handle # Cache the handle before exit so .NET retains ExitCode.
try {
  if(!$process.WaitForExit(65000)){Stop-Process -Id $process.Id;throw 'Security fixture timed out.'}
  $process.Refresh()
  @{exit_code=$process.ExitCode} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output 'exit.json')
  $receipt=Join-Path $root 'security-probe.json'
  if(!(Test-Path -LiteralPath $receipt)){throw 'No security receipt; inspect private output directory.'}
  $result=Get-Content -Raw -LiteralPath $receipt | ConvertFrom-Json
  $result | Add-Member -NotePropertyName executable_sha256 -NotePropertyValue (Get-FileHash -Algorithm SHA256 -LiteralPath $app).Hash.ToLowerInvariant()
  $result | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $output 'receipt.json')
  if(!$result.passed -or $result.popup_count -ne 0 -or $process.ExitCode -ne 0){throw 'Security fixture failed; inspect receipt.json.'}
  Write-Output 'Packaged WebView2 security fixture passed.'
}finally{$process.Dispose()}
