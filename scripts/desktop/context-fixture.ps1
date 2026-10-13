param([Parameter(Mandatory=$true)][string]$Directory)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName PresentationFramework
Add-Type -AssemblyName WindowsBase
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ContextFixtureFocus {
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window,IntPtr process);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint from,uint to,bool attach);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr window);
    public static void Activate(IntPtr window) {
        uint ours=GetCurrentThreadId(), theirs=GetWindowThreadProcessId(GetForegroundWindow(),IntPtr.Zero);
        bool attached=theirs!=0 && theirs!=ours && AttachThreadInput(ours,theirs,true);
        try {SetForegroundWindow(window);} finally {if(attached)AttachThreadInput(ours,theirs,false);}
    }
}
'@
$window=New-Object Windows.Window
$window.Title='Writer isolated context test';$window.Width=420;$window.Height=220
$panel=New-Object Windows.Controls.StackPanel
$text=New-Object Windows.Controls.TextBox
$text.Text='Tuesday then Tuesday';$text.Height=60
$password=New-Object Windows.Controls.PasswordBox
$password.Password='synthetic fixture';$password.Height=30
$panel.Children.Add($text)|Out-Null;$panel.Children.Add($password)|Out-Null;$window.Content=$panel
$encoding=New-Object Text.UTF8Encoding($false)
$window.Add_ContentRendered({
  $window.Activate()|Out-Null;$text.Focus()|Out-Null;$text.Select(0,7)
  $handle=(New-Object Windows.Interop.WindowInteropHelper($window)).Handle.ToInt64()
  [IO.File]::WriteAllText((Join-Path $Directory 'ready.json'),(@{window=$handle;process=$PID;active=$window.IsActive;visible=$window.IsVisible}|ConvertTo-Json -Compress),$encoding)
})
$timer=New-Object Windows.Threading.DispatcherTimer
$timer.Interval=[TimeSpan]::FromMilliseconds(100)
$script:last=0
$timer.Add_Tick({
  $path=Join-Path $Directory 'command.json'
  if(-not (Test-Path -LiteralPath $path)){return}
  try {$command=[IO.File]::ReadAllText($path)|ConvertFrom-Json}catch{return}
  if($command.id -le $script:last){return}
  $script:last=$command.id
  switch($command.action){
    'focus' {
      $handle=(New-Object Windows.Interop.WindowInteropHelper($window)).Handle
      [ContextFixtureFocus]::Activate($handle)
      $text.Focus()|Out-Null;$text.Select(0,7)
    }
    'other-range' {$text.Select(13,7)}
    'replace' {$text.Text='Wednesday';$text.SelectAll()}
    'password' {$password.Focus()|Out-Null}
    'close' {$window.Close()}
  }
  [IO.File]::WriteAllText((Join-Path $Directory 'ack.json'),(@{id=$command.id}|ConvertTo-Json -Compress),$encoding)
})
$timer.Start()
try {$window.ShowDialog()|Out-Null}finally{$timer.Stop()}
