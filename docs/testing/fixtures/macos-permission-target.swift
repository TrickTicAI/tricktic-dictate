// Launch this helper as a disposable .app bundle via /usr/bin/open --args <root>,
// not as a bare executable. Info.plist needs CFBundleExecutable=paste-target,
// CFBundlePackageType=APPL and a distinct test-only CFBundleIdentifier.
// Confirm NSRunningApplication.launchDate is non-nil before dictating: Writer's
// production PID/launch-time guard deliberately refuses unregistered targets.
// Output remains non-content counts/hashes; <root>/quit-fixture stops this helper.
import AppKit
import Foundation
import CryptoKit
let root=URL(fileURLWithPath:CommandLine.arguments[1])
let app=NSApplication.shared
app.setActivationPolicy(.regular)
let menu = NSMenu()
let editRoot = NSMenuItem(title:"Edit",action:nil,keyEquivalent:"")
let edit = NSMenu(title:"Edit")
edit.addItem(withTitle:"Paste",action:#selector(NSText.paste(_:)),keyEquivalent:"v")
editRoot.submenu=edit
menu.addItem(editRoot)
app.mainMenu=menu
let window=NSWindow(contentRect:NSRect(x:150,y:250,width:600,height:240),styleMask:[.titled,.closable],backing:.buffered,defer:false)
window.title="Writer permission recovery test"
let text=NSTextView(frame:NSRect(x:10,y:10,width:580,height:220))
text.font = .systemFont(ofSize:20)
window.contentView=text
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(text)
app.activate(ignoringOtherApps:true)
let timer=Timer.scheduledTimer(withTimeInterval:0.2,repeats:true){_ in
 let value:[String:Any] = ["nonempty":!text.string.isEmpty,"characters":text.string.count,"sha256":SHA256.hash(data:Data(text.string.utf8)).map{String(format:"%02x",$0)}.joined()]
 if let data=try? JSONSerialization.data(withJSONObject:value){try? data.write(to:root.appendingPathComponent("paste-target.json"),options:.atomic)}
 if FileManager.default.fileExists(atPath:root.appendingPathComponent("quit-fixture").path){app.terminate(nil)}
}
app.run()
