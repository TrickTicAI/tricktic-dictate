param([string]$Root)
$ErrorActionPreference='Stop'
Add-Type @'
using System;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;
public static class PrivateBrowserHost {
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr CreateWindowStation(string n,uint f,uint a,IntPtr s);
 [DllImport("user32.dll")] static extern IntPtr GetProcessWindowStation();
 [DllImport("user32.dll")] static extern bool SetProcessWindowStation(IntPtr s);
 [DllImport("user32.dll")] static extern bool CloseWindowStation(IntPtr s);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr h,int i,IntPtr p,uint n,out uint needed);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr CreateDesktop(string n,IntPtr d,IntPtr m,uint f,uint a,IntPtr s);
 [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr s);
 [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct Startup {public uint cb;public string reserved,desktop,title;public uint x,y,w,h,cx,cy,fill,flags;public ushort show,reserved2;public IntPtr reservedPtr,input,output,error;}
 [StructLayout(LayoutKind.Sequential)] struct ProcessInfo {public IntPtr process,thread;public uint pid,tid;}
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcess(string app,StringBuilder command,IntPtr pa,IntPtr ta,bool inherit,uint flags,IntPtr environment,string cwd,ref Startup startup,out ProcessInfo info);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode)] static extern IntPtr CreateJobObject(IntPtr a,string n);
 [DllImport("kernel32.dll")] static extern bool SetInformationJobObject(IntPtr j,int c,IntPtr p,uint n);
 [DllImport("kernel32.dll")] static extern bool AssignProcessToJobObject(IntPtr j,IntPtr p);
 [DllImport("kernel32.dll")] static extern uint ResumeThread(IntPtr h);
 [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr h,uint code);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
 static IntPtr station,desktop,job;static string address;
 static void Check(bool ok,string operation){if(!ok)throw new Exception(operation+": "+Marshal.GetLastWin32Error());}
 public static string Init(){
  Exception error=null;
  var t=new Thread(()=>{IntPtr old=GetProcessWindowStation();try{
   station=CreateWindowStation(null,0,0x37f,IntPtr.Zero);Check(station!=IntPtr.Zero,"station");
   IntPtr b=Marshal.AllocHGlobal(4096);try{uint n;Check(GetUserObjectInformation(station,1,b,12,out n),"flags");Check((Marshal.ReadInt32(b,8)&1)==0,"refuse interactive station");Check(GetUserObjectInformation(station,2,b,4096,out n),"name");string stationName=Marshal.PtrToStringUni(b);
   Check(SetProcessWindowStation(station),"bind station");string name="WriterBrowser"+Guid.NewGuid().ToString("N");desktop=CreateDesktop(name,IntPtr.Zero,IntPtr.Zero,0,0xF01FF,IntPtr.Zero);Check(desktop!=IntPtr.Zero,"desktop");address=stationName+"\\"+name;
   }finally{Marshal.FreeHGlobal(b);}
  }catch(Exception e){error=e;}finally{SetProcessWindowStation(old);}});t.Start();t.Join();if(error!=null)throw error;
  job=CreateJobObject(IntPtr.Zero,null);Check(job!=IntPtr.Zero,"job");IntPtr limits=Marshal.AllocHGlobal(144);try{Marshal.Copy(new byte[144],0,limits,144);Marshal.WriteInt32(limits,16,0x2000);Check(SetInformationJobObject(job,9,limits,144),"job limits");}finally{Marshal.FreeHGlobal(limits);}return address;
 }
 public static uint Start(string app,string args){var si=new Startup();si.cb=(uint)Marshal.SizeOf(si);si.desktop=address;ProcessInfo p;Check(CreateProcess(app,new StringBuilder("\""+app+"\" "+args),IntPtr.Zero,IntPtr.Zero,false,4,IntPtr.Zero,null,ref si,out p),"start child");try{if(!AssignProcessToJobObject(job,p.process)){TerminateProcess(p.process,1);throw new Exception("job assignment");}ResumeThread(p.thread);return p.pid;}finally{CloseHandle(p.thread);CloseHandle(p.process);}}
 public static void Close(){if(job!=IntPtr.Zero)CloseHandle(job);if(desktop!=IntPtr.Zero)CloseDesktop(desktop);if(station!=IntPtr.Zero)CloseWindowStation(station);}
}
'@
try {
 [PrivateBrowserHost]::Init() | Out-Null
 [IO.File]::WriteAllText((Join-Path $Root 'host-ready.json'),'{}')
 $deadline=[DateTime]::UtcNow.AddMinutes(5)
 while (!(Test-Path (Join-Path $Root 'stop')) -and [DateTime]::UtcNow -lt $deadline) {
  foreach($request in Get-ChildItem -LiteralPath $Root -Filter 'launch-*.json') {
   $response=$request.FullName+'.result'
   if(Test-Path -LiteralPath $response){continue}
   $r=Get-Content -Raw -LiteralPath $request.FullName | ConvertFrom-Json
   $id=[PrivateBrowserHost]::Start($r.exe,$r.args)
   [IO.File]::WriteAllText($response,(@{pid=$id}|ConvertTo-Json -Compress))
  }
  Start-Sleep -Milliseconds 25
 }
} finally {[PrivateBrowserHost]::Close()}
