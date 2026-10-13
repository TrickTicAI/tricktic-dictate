# Dedicated native field; no terminal, Enter key, external messages or private speech.
param(
 [Parameter(Mandatory=$true)][string]$Probe,
 [Parameter(Mandatory=$true)][string]$Manifest,
 [Parameter(Mandatory=$true)][string]$Model,
 [Parameter(Mandatory=$true)][string]$Ffmpeg,
 [Parameter(Mandatory=$true)][string]$Endpoint,
 [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference='Stop'
if(Test-Path $OutputDirectory){throw 'Use a fresh output directory'}
$corpus=Get-Content -Raw $Manifest | ConvertFrom-Json
if(-not $corpus.public_corpus_reuse_confirmed){throw 'Use a public corpus manifest'}
foreach($sample in $corpus.samples){if($sample.id -notmatch '^[a-zA-Z0-9_-]+$' -or -not (Test-Path $sample.audio_path)){throw 'Invalid sample ID or local WAV path'}}
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class WriterFixtureFocus {
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,UIntPtr extra);
}
'@
$script:form=New-Object Windows.Forms.Form
$form.Text='Writer isolated insertion test';$form.Width=500;$form.Height=160
$script:field=New-Object Windows.Forms.TextBox
$field.Multiline=$true;$field.Dock='Fill';$form.Controls.Add($field)
$script:index=0;$script:child=$null;$script:failed=$false;$script:receipts=@()
function Start-Trial {
 $sample=$corpus.samples[$script:index]
 $field.Clear();$form.Activate();$field.Focus() | Out-Null
 if([WriterFixtureFocus]::GetForegroundWindow() -ne $form.Handle){
  # Windows foreground locking also affects test launchers. Only bring our
  # dedicated fixture forward, before measuring. Production delivery never does this.
  [WriterFixtureFocus]::keybd_event(18,0,0,[UIntPtr]::Zero)
  [WriterFixtureFocus]::keybd_event(18,0,2,[UIntPtr]::Zero)
  [WriterFixtureFocus]::SetForegroundWindow($form.Handle) | Out-Null
 }
 $arguments=@($sample.audio_path,(Join-Path $OutputDirectory $sample.id),$Endpoint,$Model,$Ffmpeg,"$($form.Handle.ToInt64())","$($field.Handle.ToInt64())","$PID")
 $info=New-Object Diagnostics.ProcessStartInfo
 $info.FileName=$Probe;$info.Arguments=($arguments | ForEach-Object {'"'+$_+'"'}) -join ' '
 $info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
 $script:child=New-Object Diagnostics.Process;$script:child.StartInfo=$info
 $script:started=[Diagnostics.Stopwatch]::StartNew();$script:child.Start() | Out-Null
}
$script:shown=$false
$form.Add_Shown({$script:shown=$true})
$timer=New-Object Windows.Forms.Timer;$timer.Interval=100
$timer.Add_Tick({
 try {
  if($null -eq $script:child){if($script:shown){Start-Trial};return}
  if(-not $script:child.HasExited){
   if($script:started.Elapsed.TotalSeconds -gt 90){$script:child.Kill();throw 'Owned probe timed out'}
   return
  }
  $sample=$corpus.samples[$script:index]
  $script:child.StandardOutput.ReadToEnd() | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory ($sample.id+'.stdout'))
  $script:child.StandardError.ReadToEnd() | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory ($sample.id+'.stderr'))
  if($script:child.ExitCode -ne 0){throw 'Probe failed; retained diagnostics are in the output directory'}
  $receipt=Get-Content -Raw (Join-Path (Join-Path $OutputDirectory $sample.id) 'timing.json') | ConvertFrom-Json
  if(-not $receipt.inserted_exact_text){throw 'Field did not receive the transcript'}
  $script:receipts+= $receipt
  $script:child.Dispose();$script:child=$null;$script:index++
  if($script:index -ge $corpus.samples.Count){$form.Close()}else{Start-Trial}
 } catch { $script:failed=$true; $script:failure=$_.Exception.Message;$form.Close() }
})
$timer.Start()
try {[Windows.Forms.Application]::Run($form)} finally {
 $timer.Stop()
 if($null -ne $script:child -and -not $script:child.HasExited){$script:child.Kill();$script:failed=$true}
}
$receipts | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory 'results.json')
if($script:failed -or $script:index -ne $corpus.samples.Count){throw "Insertion probe incomplete: $script:failure"}
$receipts | Select-Object duration_ms,stop_to_transcript_ms,stop_to_insertion_ms,inserted_exact_text | ConvertTo-Json -Compress
