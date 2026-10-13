# Real Chromium clipboard reads without global keyboard input or the interactive
# clipboard. Use a fresh output directory and the explicitly built native probe.
param(
 [Parameter(Mandatory=$true)][string]$Probe,
 [Parameter(Mandatory=$true)][string]$XtermDirectory,
 [Parameter(Mandatory=$true)][string]$OutputDirectory,
 [string]$Browser='C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe',
 [string]$Node='C:\Program Files\nodejs\node.exe'
)
$ErrorActionPreference='Stop'
if(Test-Path -LiteralPath $OutputDirectory){throw 'Preserve prior receipts; choose a fresh output directory.'}
foreach($path in @($Probe,$Browser,$Node,(Join-Path $XtermDirectory 'lib/xterm.js'),(Join-Path $XtermDirectory 'css/xterm.css'))){if(!(Test-Path -LiteralPath $path -PathType Leaf)){throw ('Missing fixture input: '+$path)}}
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
Copy-Item -LiteralPath $Probe -Destination (Join-Path $OutputDirectory 'clipboard-probe.exe')
Copy-Item -LiteralPath (Join-Path $XtermDirectory 'lib/xterm.js') -Destination $OutputDirectory
Copy-Item -LiteralPath (Join-Path $XtermDirectory 'css/xterm.css') -Destination $OutputDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'clipboard-private-host.ps1') -Destination (Join-Path $OutputDirectory 'host.ps1')
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'clipboard-private-browser.mjs') -Destination (Join-Path $OutputDirectory 'check.mjs')
$arguments=@('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',('"'+(Join-Path $OutputDirectory 'host.ps1')+'"'),'-Root',('"'+$OutputDirectory+'"'))
$hostProcess=Start-Process powershell.exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $OutputDirectory 'host.stdout.txt') -RedirectStandardError (Join-Path $OutputDirectory 'host.stderr.txt')
try {
 & $Node (Join-Path $OutputDirectory 'check.mjs') $OutputDirectory $Browser
 if($LASTEXITCODE -ne 0){throw 'Private browser clipboard validation failed.'}
 $receipts=Get-Content -Raw (Join-Path $OutputDirectory 'brave-receipts.json') | ConvertFrom-Json
 if($receipts.Count -ne 18 -or @($receipts|Where-Object {!$_.passed}).Count){throw 'Missing or failed clipboard cases.'}
 Write-Output 'All 18 private browser clipboard cases passed.'
} finally {
 [IO.File]::WriteAllText((Join-Path $OutputDirectory 'stop'),'')
 if(!$hostProcess.WaitForExit(10000)){Stop-Process -Id $hostProcess.Id}
 # The host's kill-on-close job contains only the fixture processes.
}
