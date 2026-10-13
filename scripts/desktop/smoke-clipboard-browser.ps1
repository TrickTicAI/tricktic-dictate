param(
    [Parameter(Mandatory=$true)][string]$Probe,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [Parameter(Mandatory=$true)][string]$FixtureUrl,
    [Parameter(Mandatory=$true)][string]$FixtureTitle,
    [switch]$Interactive
)
$ErrorActionPreference='Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class FixtureWindow {
    public delegate bool EnumProc(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr data);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr window,StringBuilder text,int length);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window,out uint process);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window,uint message,IntPtr wp,IntPtr lp);
    public static IntPtr Find(string expectedTitle) {
        IntPtr found=IntPtr.Zero;
        EnumWindows((window,data)=>{
            var title=new StringBuilder(256); GetWindowText(window,title,256);
            if(title.ToString().StartsWith(expectedTitle)) {found=window;return false;}
            return true;
        },IntPtr.Zero);
        return found;
    }
}
'@
$edge=Join-Path ${env:ProgramFiles(x86)} 'Microsoft\Edge\Application\msedge.exe'
$url=$FixtureUrl
$profile=Join-Path $OutputDirectory 'edge-profile'
$child=Start-Process $edge -ArgumentList @('--no-first-run','--no-default-browser-check',('--user-data-dir="'+$profile+'"'),('--app="'+$url+'"')) -PassThru
$window=[IntPtr]::Zero
try {
    $deadline=[DateTime]::UtcNow.AddSeconds(20)
    while ($window -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 100; $window=[FixtureWindow]::Find($FixtureTitle)
    }
    if($window -eq [IntPtr]::Zero){throw 'Isolated Edge fixture did not start.'}
    [uint32]$targetProcess=0
    [void][FixtureWindow]::GetWindowThreadProcessId($window,[ref]$targetProcess)
    $targetPath=Join-Path $OutputDirectory 'target-browser.json'
    @{window=$window.ToInt64();process=$targetProcess} | ConvertTo-Json | Set-Content -Encoding Ascii $targetPath
    if($Interactive){
        Write-Output 'Ready: click Run clipboard check in the Writer clipboard check browser window.'
        $ready=Join-Path $OutputDirectory 'start-browser.json'
        $deadline=[DateTime]::UtcNow.AddMinutes(20)
        while ((!(Test-Path $ready) -or [FixtureWindow]::GetForegroundWindow() -ne $window) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
        if(!(Test-Path $ready) -or [FixtureWindow]::GetForegroundWindow() -ne $window){throw 'The interactive check was not started.'}
    } else { Start-Sleep -Milliseconds 500 }
    $probeProcess=Start-Process -FilePath $Probe -ArgumentList @(('"'+$targetPath+'"'),'browser') -Wait -PassThru
    $code=$probeProcess.ExitCode
    $result=Get-Content -Raw -Encoding UTF8 (Join-Path $OutputDirectory 'delivery-browser.json')
    $result | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory 'browser.json')
    $result
    if($Interactive){Start-Sleep -Seconds 3}
    if($code -ne 0){throw 'Browser clipboard acceptance failed.'}
} finally {
    if($window -ne [IntPtr]::Zero){
        [uint32]$currentProcess=0
        [void][FixtureWindow]::GetWindowThreadProcessId($window,[ref]$currentProcess)
        if($currentProcess -eq $targetProcess){[void][FixtureWindow]::PostMessage($window,0x10,[IntPtr]::Zero,[IntPtr]::Zero)}
    }
}
