// Read the actual Unicode Shell Link strings; WScript's automation getter can
// return ANSI replacement characters even for a correct Unicode .lnk file.
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;
public static class WriterShortcutReader {
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")] class Link {}
    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface Reader {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder value,int count,IntPtr data,uint flags);
        void GetIDList(out IntPtr list); void SetIDList(IntPtr list);
        void GetDescription([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder value,int count);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string value);
        void GetWorkingDirectory([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder value,int count);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string value);
        void GetArguments([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder value,int count);
    }
    public sealed class Entry { public string TargetPath; public string Arguments; }
    public static Entry Read(string path) {
        object instance=new Link();
        try {
            ((IPersistFile)instance).Load(path,0);
            var reader=(Reader)instance;
            var target=new StringBuilder(32768); var arguments=new StringBuilder(32768);
            reader.GetPath(target,target.Capacity,IntPtr.Zero,0);
            reader.GetArguments(arguments,arguments.Capacity);
            return new Entry { TargetPath=target.ToString(),Arguments=arguments.ToString() };
        } finally { Marshal.FinalReleaseComObject(instance); }
    }
}
