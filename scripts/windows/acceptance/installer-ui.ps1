# MW-RELEASE-10 native installer screenshots, restricted to the disposable account.
param([ValidateSet('install','uninstall')][string]$Mode, [string]$Shared = 'C:\Users\Public\mwr10')
$ErrorActionPreference = 'Stop'
if ($env:USERNAME -ne 'dictate-test') { throw 'Run only as dictate-test.' }
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class InstallerWindow {
 public delegate bool EnumProc(IntPtr h, IntPtr l);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
 [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumProc cb, IntPtr l);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
 [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 public struct Rect { public int L,T,R,B; }
 public static string Text(IntPtr h) { var s=new StringBuilder(2048);GetWindowText(h,s,s.Capacity);return s.ToString(); }
 public static IntPtr[] All(IntPtr parent) { var a=new List<IntPtr>(); EnumProc f=(h,l)=>{a.Add(h);return true;};if(parent==IntPtr.Zero)EnumWindows(f,IntPtr.Zero);else EnumChildWindows(parent,f,IntPtr.Zero);return a.ToArray(); }
}
'@
function Entry { Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -eq 'TrickTic Dictate' } | Select-Object -First 1 }
function Window {
 foreach ($h in [InstallerWindow]::All([IntPtr]::Zero)) {
  if (![InstallerWindow]::IsWindowVisible($h) -or [InstallerWindow]::Text($h) -notmatch 'TrickTic Dictate (Setup|Uninstall)') { continue }
  [uint32]$procId=0;[void][InstallerWindow]::GetWindowThreadProcessId($h,[ref]$procId)
  $proc=Get-CimInstance Win32_Process -Filter "ProcessId=$procId"
  if ($proc -and (Invoke-CimMethod -InputObject $proc -MethodName GetOwner).User -eq $env:USERNAME) { return $h }
 }
 return [IntPtr]::Zero
}
function Shot($h,$file) {
 $rect=New-Object InstallerWindow+Rect;[void][InstallerWindow]::GetWindowRect($h,[ref]$rect)
 $bitmap=New-Object Drawing.Bitmap(($rect.R-$rect.L),($rect.B-$rect.T));$graphics=[Drawing.Graphics]::FromImage($bitmap);$dc=$graphics.GetHdc()
 try { if (![InstallerWindow]::PrintWindow($h,$dc,2)) { throw 'PrintWindow failed' } } finally { $graphics.ReleaseHdc($dc);$graphics.Dispose() }
 $bitmap.Save($file,[Drawing.Imaging.ImageFormat]::Png);$bitmap.Dispose()
}
$receipt=[ordered]@{schema=1;kind='mw-release-10-installer-ui';mode=$Mode;started=(Get-Date).ToUniversalTime().ToString('o');screens=@()}
try {
 Get-Process tricktic-dictate,metrix-writer-desktop -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
 Start-Sleep 3
 $entry=Entry
 if ($Mode -eq 'install') { if ($entry) { throw 'Fresh UI install requires no existing registration.' };$exe=Join-Path $Shared 'candidate.exe' }
 else { if (!$entry) { throw 'No installation to uninstall.' };$exe=$entry.UninstallString.Trim('"') }
 $receipt.installer_sha256=(Get-FileHash (Join-Path $Shared 'candidate.exe')).Hash.ToLowerInvariant()
 Start-Process -FilePath $exe | Out-Null
 $deadline=(Get-Date).AddMinutes(3);$i=0;$seen=$false
 while ((Get-Date) -lt $deadline) {
  Start-Sleep 2
  $h=Window
  if ($h -eq [IntPtr]::Zero) { if ($seen) { break };continue }
  $seen=$true;$i++
  $children=@([InstallerWindow]::All($h) | Where-Object { [InstallerWindow]::IsWindowVisible($_) })
  $texts=@($children | ForEach-Object { [InstallerWindow]::Text($_) } | Where-Object { $_ })
  $file="ui-$Mode-$i.png";Shot $h (Join-Path $Shared "out\$file")
  $receipt.screens+=@{file=$file;text=$texts}
  $button=$children | Where-Object { [InstallerWindow]::IsWindowEnabled($_) -and [InstallerWindow]::Text($_) -match '^(&?Next\s*>?|&?Install|&?Uninstall|&?Finish|&?Close)$' } | Select-Object -First 1
  if ($button) { if (![InstallerWindow]::PostMessage($h,0x111,[IntPtr]([InstallerWindow]::GetDlgCtrlID($button)),$button)) { throw 'Could not post installer button click.' } }
 }
 Start-Sleep 4
 $entry=Entry
 $receipt.registered_version=if ($entry) { $entry.DisplayVersion } else { $null }
 $receipt.install_directory_exists=Test-Path (Join-Path $env:LOCALAPPDATA 'Programs\TrickTic Dictate')
 $receipt.data_retained=Test-Path (Join-Path $env:LOCALAPPDATA 'com.tricktic.dictate')
 $receipt.passed=$seen -and $(if ($Mode -eq 'install') { $receipt.registered_version -eq '0.24.0' } else { !$entry -and !$receipt.install_directory_exists })
} catch { $receipt.error=$_.Exception.Message;$receipt.passed=$false }
finally { $receipt.finished=(Get-Date).ToUniversalTime().ToString('o');$receipt | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $Shared "out\ui-$Mode.json") -Encoding UTF8 }
if (!$receipt.passed) { exit 1 }
