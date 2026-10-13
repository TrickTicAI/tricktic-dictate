param([Parameter(Mandatory=$true)][string]$TestExecutable,[Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
$directory=[IO.Path]::GetFullPath($OutputDirectory)
if(Test-Path -LiteralPath $directory){throw 'Use a new private output directory.'}
New-Item -ItemType Directory -Path $directory|Out-Null
$fixture=Join-Path $PSScriptRoot 'context-fixture.ps1'
$worker=Start-Process powershell.exe -ArgumentList @('-NoProfile','-Sta','-ExecutionPolicy','Bypass','-File',('"'+$fixture+'"'),'-Directory',('"'+$directory+'"')) -WindowStyle Hidden -PassThru
$previous=$env:WRITER_CONTEXT_FIXTURE_DIR
try {
  $until=[DateTime]::UtcNow.AddSeconds(20)
  while(-not(Test-Path -LiteralPath (Join-Path $directory 'ready.json')) -and [DateTime]::UtcNow -lt $until -and -not $worker.HasExited){Start-Sleep -Milliseconds 100}
  if(-not(Test-Path -LiteralPath (Join-Path $directory 'ready.json'))){throw 'Context fixture did not become ready.'}
  $env:WRITER_CONTEXT_FIXTURE_DIR=$directory
  $start=New-Object Diagnostics.ProcessStartInfo
  $start.FileName=$TestExecutable;$start.UseShellExecute=$false;$start.CreateNoWindow=$true
  $start.Arguments='shell::context::tests::native_selection_rejects_moved_ranges_edits_and_password_fields --ignored --exact'
  $start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
  $test=New-Object Diagnostics.Process;$test.StartInfo=$start
  try {
    $test.Start()|Out-Null;$stdout=$test.StandardOutput.ReadToEndAsync();$stderr=$test.StandardError.ReadToEndAsync()
    if(-not $test.WaitForExit(50000)){$test.Kill();throw 'Native context test timed out.'}
    [IO.File]::WriteAllText((Join-Path $directory 'tests.log'),$stdout.GetAwaiter().GetResult()+$stderr.GetAwaiter().GetResult())
    if($test.ExitCode -ne 0){throw 'Native context tests failed; inspect private test log.'}
  }finally{$test.Dispose()}
  @{passed=$true;scope='WPF selection, identical text at another range, edited target, session identity and password rejection';test_executable_sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $TestExecutable).Hash.ToLowerInvariant()} | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $directory 'receipt.json')
  Write-Output 'Native context and stale-target checks passed.'
}finally{
  $env:WRITER_CONTEXT_FIXTURE_DIR=$previous
  if(-not $worker.HasExited){Stop-Process -Id $worker.Id}
  $worker.Dispose()
}
