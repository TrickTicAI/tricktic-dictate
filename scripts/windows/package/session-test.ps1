param([Parameter(Mandatory=$true)][string]$PackageDirectory, [Parameter(Mandatory=$true)][string]$ShellDirectory, [Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$root = Join-Path $OutputDirectory ('session-' + [Guid]::NewGuid().ToString('N'))
$fixture = Join-Path $PSScriptRoot 'fixture.js'
$stdout = Join-Path $OutputDirectory 'integrated.stdout.txt'
$stderr = Join-Path $OutputDirectory 'integrated.stderr.txt'
$p = Start-Process -FilePath (Join-Path $ShellDirectory 'MetrixWriter.Smoke.exe') -ArgumentList @('--integrated', ('"'+$PackageDirectory+'"'), ('"'+$root+'"'), ('"'+$fixture+'"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
$p.Handle | Out-Null
if (!$p.WaitForExit(60000)) { $p.Kill(); throw 'Integrated fixture exceeded 60 seconds.' }
$p.WaitForExit(); Get-Content $stdout
if ($p.ExitCode -ne 0) { Get-Content $stderr; throw 'Integrated fixtures failed.' }
