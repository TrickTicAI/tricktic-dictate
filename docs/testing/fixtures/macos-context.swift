import AppKit
import Foundation
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let root = URL(fileURLWithPath:"/tmp/mw50-context-fixture")
try? FileManager.default.createDirectory(at:root,withIntermediateDirectories:true)
let window = NSWindow(contentRect:NSRect(x:160,y:230,width:680,height:320),styleMask:[.titled,.closable],backing:.buffered,defer:false)
window.title = "MW-50 automatic context check"
let container = NSView(frame:NSRect(x:0,y:0,width:680,height:320))
let text = NSTextView(frame:NSRect(x:20,y:100,width:640,height:180))
text.font = .systemFont(ofSize:25)
let phrase = "שלום Writer"
text.string = phrase + " / " + phrase
let secret = NSSecureTextField(frame:NSRect(x:20,y:30,width:640,height:35))
secret.stringValue = "synthetic fixture password"
container.addSubview(text);container.addSubview(secret);window.contentView=container
window.makeKeyAndOrderFront(nil);window.makeFirstResponder(text)
text.setSelectedRange(NSRange(location:0,length:(phrase as NSString).length))
app.activate(ignoringOtherApps:true)
let board=NSPasteboard.general;board.clearContents();board.setString("MW-50 clipboard fixture",forType:.string)
var last = 0
let timer = Timer.scheduledTimer(withTimeInterval:0.02,repeats:true){_ in
 guard let bytes=try? Data(contentsOf:root.appendingPathComponent("command.json")),let command=try? JSONSerialization.jsonObject(with:bytes) as? [String:Any],let id=command["id"] as? Int,id>last,let action=command["action"] as? String else{return}
 last=id
 switch action {
 case "other-range": text.setSelectedRange(NSRange(location:(phrase as NSString).length+3,length:(phrase as NSString).length))
 case "replace": text.string="different selection";text.setSelectedRange(NSRange(location:0,length:9))
 case "password":window.makeFirstResponder(secret);secret.selectText(nil)
 case "normal":window.makeFirstResponder(text)
 case "quit":app.terminate(nil)
 default:break
 }
 if let ack=try? JSONSerialization.data(withJSONObject:["id":id]){try? ack.write(to:root.appendingPathComponent("ack.json"),options:.atomic)}
}
app.run()
