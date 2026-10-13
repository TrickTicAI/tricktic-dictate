# MW-SEC-21: run one acceptance script as a disposable standard Windows account.
# The password is read from an owner-only file and never printed or passed on a
# command line. Run from a normal (non-elevated) session of the machine owner.
param(
    [Parameter(Mandatory=$true)][string]$User,
    [Parameter(Mandatory=$true)][string]$PasswordFile,
    [Parameter(Mandatory=$true)][string]$Script,
    [int]$TimeoutSeconds = 3600,
    # Everything after the named parameters is passed to -Script unchanged.
    [Parameter(ValueFromRemainingArguments=$true)][string[]]$ScriptArguments = @()
)
$ErrorActionPreference = 'Stop'
$password = ConvertTo-SecureString -String ([IO.File]::ReadAllText($PasswordFile).Trim()) -AsPlainText -Force
$credential = New-Object Management.Automation.PSCredential(".\$User", $password)
$arguments = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', ('"' + $Script + '"')) + ($ScriptArguments | ForEach-Object { '"' + $_ + '"' })
$process = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -ArgumentList $arguments `
    -Credential $credential -LoadUserProfile -WorkingDirectory $env:SystemRoot -WindowStyle Hidden -PassThru
$null = $process.Handle
if (!$process.WaitForExit($TimeoutSeconds * 1000)) { Stop-Process -Id $process.Id -Force; throw "Timed out after $TimeoutSeconds s" }
exit $process.ExitCode
