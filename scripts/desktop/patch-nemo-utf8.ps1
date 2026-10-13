param([Parameter(Mandatory=$true)][string]$InputDirectory,[Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
if(Test-Path -LiteralPath $OutputDirectory){throw 'Use a new output directory; source packages are never modified.'}
$originalHashes=@{cpu='1e2110e2a39c789f0c3bd49eea7898dfe62591d4dbebb73f4cb549a5b516a8e5';vulkan='1532542b5519d06c41dbbe74891a41df4eadd2f0314881ff68e0ad11e2db55bf'}
foreach($backend in @('cpu','vulkan')) {
  if((Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $InputDirectory "$backend/bin/nemo-speech.exe")).Hash.ToLowerInvariant() -ne $originalHashes[$backend]) { throw 'The launcher does not match the pinned upstream release.' }
}
New-Item -ItemType Directory -Path $OutputDirectory|Out-Null
foreach($backend in @('cpu','vulkan')) { Copy-Item -LiteralPath (Join-Path $InputDirectory $backend) -Destination (Join-Path $OutputDirectory $backend) -Recurse }
Add-Type -ReferencedAssemblies @('System.dll','System.Core.dll','System.Xml.dll') -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using System.Xml;
public static class WriterManifest {
 [DllImport("kernel32",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr LoadLibraryEx(string file,IntPtr reserved,uint flags);
 [DllImport("kernel32",SetLastError=true)] static extern bool FreeLibrary(IntPtr module);
 delegate bool Languages(IntPtr module,IntPtr type,IntPtr name,ushort language,IntPtr param);
 [DllImport("kernel32",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool EnumResourceLanguages(IntPtr module,IntPtr type,IntPtr name,Languages callback,IntPtr param);
 [DllImport("kernel32",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr FindResourceEx(IntPtr module,IntPtr type,IntPtr name,ushort language);
 [DllImport("kernel32",SetLastError=true)] static extern IntPtr LoadResource(IntPtr module,IntPtr resource);
 [DllImport("kernel32",SetLastError=true)] static extern IntPtr LockResource(IntPtr resource);
 [DllImport("kernel32",SetLastError=true)] static extern uint SizeofResource(IntPtr module,IntPtr resource);
 [DllImport("kernel32",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr BeginUpdateResource(string file,bool deleteExisting);
 [DllImport("kernel32",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool UpdateResource(IntPtr update,IntPtr type,IntPtr name,ushort language,byte[] data,uint bytes);
 [DllImport("kernel32",SetLastError=true)] static extern bool EndUpdateResource(IntPtr update,bool discard);
 public static void Patch(string file) {
  var type=new IntPtr(24);var name=new IntPtr(1);var manifests=new Dictionary<ushort,byte[]>();
  var module=LoadLibraryEx(file,IntPtr.Zero,2);
  if(module==IntPtr.Zero)throw new Win32Exception();
  try {
   Languages callback=(m,t,n,l,p)=>{
    var resource=FindResourceEx(m,t,n,l);var bytes=new byte[SizeofResource(m,resource)];
    Marshal.Copy(LockResource(LoadResource(m,resource)),bytes,0,bytes.Length);manifests.Add(l,bytes);return true;
   };
   EnumResourceLanguages(module,type,name,callback,IntPtr.Zero);
  }finally{FreeLibrary(module);}
  if(manifests.Count==0)throw new Exception("Expected the pinned launcher's existing manifest.");
  var update=BeginUpdateResource(file,false);if(update==IntPtr.Zero)throw new Win32Exception();
  bool committed=false;
  try {
   foreach(var pair in manifests) {
    var xml=new XmlDocument();xml.XmlResolver=null;xml.PreserveWhitespace=true;
    xml.LoadXml(Encoding.UTF8.GetString(pair.Value).TrimEnd('\0'));
    var ns=new XmlNamespaceManager(xml.NameTable);ns.AddNamespace("v3","urn:schemas-microsoft-com:asm.v3");
    var app=xml.DocumentElement.SelectSingleNode("v3:application",ns);
    if(app==null){app=xml.CreateElement("application","urn:schemas-microsoft-com:asm.v3");xml.DocumentElement.AppendChild(app);}
    var settings=app.SelectSingleNode("v3:windowsSettings",ns);
    if(settings==null){settings=xml.CreateElement("windowsSettings","urn:schemas-microsoft-com:asm.v3");app.AppendChild(settings);}
    var codepage=xml.CreateElement("activeCodePage","http://schemas.microsoft.com/SMI/2019/WindowsSettings");codepage.InnerText="UTF-8";settings.AppendChild(codepage);
    var data=new UTF8Encoding(false).GetBytes(xml.OuterXml);
    if(!UpdateResource(update,type,name,pair.Key,data,(uint)data.Length))throw new Win32Exception();
   }
   if(!EndUpdateResource(update,false))throw new Win32Exception();committed=true;
  }finally{if(!committed)EndUpdateResource(update,true);}
 }
}
'@
foreach($backend in @('cpu','vulkan')) {
  $file=(Resolve-Path -LiteralPath (Join-Path $OutputDirectory "$backend/bin/nemo-speech.exe")).ProviderPath
  [WriterManifest]::Patch($file)
}
Write-Output 'Patched NeMo copies with UTF-8 process manifests; upstream sources retained.'
