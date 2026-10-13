# Build with --features custom-protocol,latency-probe. Root must be an isolated
# prepared data root; Sample must be an explicitly selected public PCM16 fixture.
param(
 [Parameter(Mandatory=$true)][string]$Executable,
 [Parameter(Mandatory=$true)][string]$Root,
 [Parameter(Mandatory=$true)][string]$Sample,
 [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference='Stop'
if(Test-Path $OutputDirectory){throw 'Use a fresh output directory'}
if((Test-Path (Join-Path $Root 'Data')) -or -not (Test-Path (Join-Path $Root 'State/settings.json'))){throw 'Use a freshly prepared isolated root with no recording history'}
New-Item -ItemType Directory $OutputDirectory | Out-Null
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class WriterReplayFocus {
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
}
'@
$form=New-Object Windows.Forms.Form
$form.Text='Writer isolated dictation acceptance';$form.Width=600;$form.Height=200
$field=New-Object Windows.Forms.TextBox
$field.Multiline=$true;$field.Dock='Fill';$form.Controls.Add($field)
$script:child=$null;$script:failed=$false;$script:receipt=$null
$form.Add_Shown({
 try {
  $field.Focus() | Out-Null
  [WriterReplayFocus]::SetForegroundWindow($form.Handle) | Out-Null
  [IO.File]::WriteAllText((Join-Path $Root 'fixture-target.json'),("["+$form.Handle.ToInt64()+","+$PID+"]"))
  $info=New-Object Diagnostics.ProcessStartInfo
  $info.FileName=$Executable;$info.Arguments='--dictation-probe "'+$Root+'" "'+$Sample+'"'
  $info.UseShellExecute=$false;$info.CreateNoWindow=$true
  $info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
  $script:child=New-Object Diagnostics.Process;$script:child.StartInfo=$info
  $script:watch=[Diagnostics.Stopwatch]::StartNew();$script:child.Start() | Out-Null
 } catch {$script:failed=$true;$form.Close()}
})
$timer=New-Object Windows.Forms.Timer;$timer.Interval=50
$timer.Add_Tick({
 try {
  if($null -eq $script:child){return}
  if(-not $script:child.HasExited){
   if($script:watch.Elapsed.TotalSeconds -gt 140){$script:child.Kill();throw 'Owned replay exceeded deadline'}
   return
  }
  $script:child.StandardOutput.ReadToEnd() | Set-Content (Join-Path $OutputDirectory 'stdout.log')
  $script:child.StandardError.ReadToEnd() | Set-Content (Join-Path $OutputDirectory 'stderr.log')
  if($script:child.ExitCode -ne 0){throw 'Replay failed; inspect private probe.json'}
  $probe=Get-Content -Raw (Join-Path $Root 'probe.json') | ConvertFrom-Json
  if(@($probe.PSObject.Properties | Where-Object {$_.Value -ne $true}).Count){throw 'App probe did not pass'}
  $jobFile=Get-ChildItem (Join-Path $Root 'Data/jobs') -Filter '*.json' | Select-Object -First 1
  # The job file is UTF-8 without a byte-order mark; Windows PowerShell would
  # otherwise read Hebrew as the ANSI code page and never match the field.
  $job=Get-Content -Raw -Encoding UTF8 $jobFile.FullName | ConvertFrom-Json
  $expected=$job.status.result.transcript.raw_text
  $exact=$field.Text -ceq $expected
  $normalized=$field.Text.Replace("`r`n","`n") -ceq $expected.Replace("`r`n","`n")
  [IO.File]::WriteAllText((Join-Path $OutputDirectory 'field.txt'),$field.Text)
  $delivery=Get-Content -Raw (Join-Path $Root 'State/last-delivery.json') | ConvertFrom-Json
  $webview=Get-Content -Raw (Join-Path $Root 'State/webview-latency.json') | ConvertFrom-Json
  $script:receipt=@{field_exact=$exact;field_newline_normalized=$normalized;delivery=$delivery;webview=$webview;probe=$probe}
  if(-not $normalized){throw 'Dedicated field did not receive the saved transcript'}
  $form.Close()
 } catch { $script:failed=$true;$script:failure=$_.Exception.Message;$form.Close() }
})
$timer.Start()
try {[Windows.Forms.Application]::Run($form)} finally {
 $timer.Stop()
 if($null -ne $script:child -and -not $script:child.HasExited){$script:child.Kill();$script:failed=$true}
}
$script:receipt | ConvertTo-Json -Depth 12 | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory 'result.json')
if($script:failed){throw "Native WebView acceptance failed: $script:failure"}
@{field_exact=$receipt.field_exact;field_newline_normalized=$receipt.field_newline_normalized;stop_to_delivery_ms=$receipt.delivery.stop_to_delivery_ms;webview_ms=$receipt.webview.stop_command_to_receipt_ms} | ConvertTo-Json -Compress
