param([int]$ProcessId, [string]$UserRoot, [ValidateSet('Inspect','Backdrop','Redraw','Bury')][string]$Action='Inspect')
$ErrorActionPreference='Stop'
# Mutating probes are restricted to the smoke's private child, never the installed app.
$child=Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId"
if (!$UserRoot -or !$child.CommandLine.Contains('--user-root') -or !$child.CommandLine.Contains($UserRoot) -or !(Split-Path $UserRoot -Leaf).StartsWith('writer-panel-recovery-')) { throw 'Expected the isolated panel smoke child' }
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing @'
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using System.Drawing;
public static class PanelProbe {
 public delegate bool Callback(IntPtr hwnd, IntPtr arg);
 [StructLayout(LayoutKind.Sequential)] public struct Rect { public int left,top,right,bottom; }
 [DllImport("user32.dll")] public static extern bool EnumWindows(Callback callback, IntPtr arg);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd,StringBuilder text,int length);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd,out Rect rect);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
 [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
 [DllImport("user32.dll")] public static extern IntPtr SetProcessDpiAwarenessContext(IntPtr context);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern int GetWindowRgn(IntPtr hwnd,IntPtr region);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd,IntPtr after,int x,int y,int width,int height,uint flags);
 [DllImport("user32.dll")] public static extern bool RedrawWindow(IntPtr hwnd,IntPtr rect,IntPtr region,uint flags);
 [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr hwnd);
 [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr hwnd,IntPtr dc);
 [DllImport("gdi32.dll")] public static extern uint GetPixel(IntPtr dc,int x,int y);
 [DllImport("gdi32.dll")] public static extern IntPtr CreateRectRgn(int left,int top,int right,int bottom);
 [DllImport("gdi32.dll")] public static extern bool PtInRegion(IntPtr region,int x,int y);
 [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr obj);
 [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr hwnd,uint command);
 [DllImport("user32.dll",EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLong(IntPtr hwnd,int index);
 public static bool Topmost(IntPtr hwnd) { return (GetWindowLong(hwnd,-20).ToInt64() & 8) != 0; }
 // An ordinary (non-topmost) visible window that overlaps the panel and precedes it in the Z order.
 public static bool Covered(IntPtr hwnd) {
  Rect panel; if (!IsWindowVisible(hwnd) || !GetWindowRect(hwnd,out panel)) return false;
  for (IntPtr above=GetWindow(hwnd,3); above!=IntPtr.Zero; above=GetWindow(above,3)) {
   Rect r; if (!IsWindowVisible(above) || IsIconic(above) || Topmost(above) || !GetWindowRect(above,out r)) continue;
   if (r.left<panel.right && panel.left<r.right && r.top<panel.bottom && panel.top<r.bottom) return true;
  }
  return false;
 }
 public static IntPtr Find(uint process) {
  IntPtr found=IntPtr.Zero;
  EnumWindows((hwnd,arg)=> {uint pid;GetWindowThreadProcessId(hwnd,out pid);if(pid==process){var title=new StringBuilder(256);GetWindowText(hwnd,title,256);if(title.ToString().EndsWith(" recording")){found=hwnd;return false;}}return true;},IntPtr.Zero);
  return found;
 }
 public sealed class Backdrop : Form {
  protected override bool ShowWithoutActivation { get { return true; } }
  protected override CreateParams CreateParams { get { var p=base.CreateParams; p.ExStyle|=0x08000000; return p; } }
  public Backdrop(Rect r) { FormBorderStyle=FormBorderStyle.None;ShowInTaskbar=false;BackColor=Color.Black;StartPosition=FormStartPosition.Manual;Bounds=new Rectangle(r.left-20,r.top-20,r.right-r.left+40,r.bottom-r.top+40); }
 }
}
'@
[void][PanelProbe]::SetProcessDpiAwarenessContext([IntPtr](-4))
$hwnd=[PanelProbe]::Find($ProcessId)
if ($hwnd -eq [IntPtr]::Zero) { throw 'Private panel missing' }
$rect=New-Object PanelProbe+Rect
[void][PanelProbe]::GetWindowRect($hwnd,[ref]$rect)
if ($Action -eq 'Bury') {
 # An ordinary window over the panel, then the panel moved beneath it, as observed on 2026-10-22.
 $form=New-Object PanelProbe+Backdrop($rect)
 $form.BackColor=[Drawing.Color]::FromArgb(40,120,60)
 $timer=New-Object Windows.Forms.Timer
 $timer.Interval=120000
 $timer.Add_Tick({$form.Close()})
 $timer.Start()
 $form.Add_Shown({[void][PanelProbe]::SetWindowPos($hwnd,$form.Handle,0,0,0,0,0x13);[Console]::WriteLine('READY buried=' + [PanelProbe]::Covered($hwnd))})
 [Windows.Forms.Application]::Run($form)
 exit
}
if ($Action -eq 'Backdrop') {
 $form=New-Object PanelProbe+Backdrop($rect)
 $timer=New-Object Windows.Forms.Timer
 $timer.Interval=120000
 $timer.Add_Tick({$form.Close()})
 $timer.Start()
 $form.TopMost=$true
 $form.Add_Shown({[void][PanelProbe]::SetWindowPos($form.Handle,$hwnd,0,0,0,0,0x13);[Console]::WriteLine('READY')})
 [Windows.Forms.Application]::Run($form)
 exit
}
if ($Action -eq 'Redraw') { [void][PanelProbe]::RedrawWindow($hwnd,[IntPtr]::Zero,[IntPtr]::Zero,0x181) }
$region=[PanelProbe]::CreateRectRgn(0,0,0,0)
$kind=[PanelProbe]::GetWindowRgn($hwnd,$region)
$width=$rect.right-$rect.left
$height=$rect.bottom-$rect.top
$clipped=$kind -ne 0 -and ![PanelProbe]::PtInRegion($region,1,1)
$centre=$kind -ne 0 -and [PanelProbe]::PtInRegion($region,[int]($width/2),[int]($height/2))
$badge=$kind -ne 0 -and [PanelProbe]::PtInRegion($region,($width-8),2)
[void][PanelProbe]::DeleteObject($region)
$dc=[PanelProbe]::GetDC([IntPtr]::Zero)
$black=$true
foreach ($across in @(35,65)) { foreach ($down in @(18,82)) {
 $pixel=[PanelProbe]::GetPixel($dc,($rect.left+[int]($width*$across/100)),($rect.top+[int]($height*$down/100)))
 $black=$black -and $pixel -ne [uint32]::MaxValue -and (($pixel -band 255) -le 12) -and ((($pixel -shr 8) -band 255) -le 12) -and ((($pixel -shr 16) -band 255) -le 12)
}}
[void][PanelProbe]::ReleaseDC([IntPtr]::Zero,$dc)
$foregroundProcess=0
[void][PanelProbe]::GetWindowThreadProcessId([PanelProbe]::GetForegroundWindow(),[ref]$foregroundProcess)
@{coveredByNormal=[PanelProbe]::Covered($hwnd);topmost=[PanelProbe]::Topmost($hwnd);foregroundBelongsToApp=($foregroundProcess -eq $ProcessId);found=$true;visible=[PanelProbe]::IsWindowVisible($hwnd);minimized=[PanelProbe]::IsIconic($hwnd);cornerClipped=$clipped;centreIncluded=$centre;badgeIncluded=$badge;fillLooksBlack=$black;width=$width;height=$height;dpi=[PanelProbe]::GetDpiForWindow($hwnd);foreground=[PanelProbe]::GetForegroundWindow().ToInt64()} | ConvertTo-Json -Compress
