// Inspect/click only the disposable same-identity acceptance app, by PID.
// AppleScript references lose PID disambiguation when app names are duplicated.
import AppKit
import ApplicationServices
import Foundation

let args = CommandLine.arguments
if args.count < 3 { fatalError("usage: ax <private-fixture-pid> list|click [label] [index]") }
guard let pid = Int32(args[1]),
      let executable = NSRunningApplication(processIdentifier: pid)?.executableURL?.path,
      executable.hasPrefix(FileManager.default.currentDirectoryPath + "/.local/native-"),
      executable.contains("/same-identity/install/TrickTic Dictate.app/Contents/MacOS/") else {
    fatalError("Refusing a process outside this worktree's private same-identity fixture")
}
let app = AXUIElementCreateApplication(pid)
func value(_ e: AXUIElement, _ name: String) -> CFTypeRef? {
    var result: CFTypeRef?
    return AXUIElementCopyAttributeValue(e, name as CFString, &result) == .success ? result : nil
}
func string(_ e: AXUIElement, _ name: String) -> String { value(e, name) as? String ?? "" }
var items: [AXUIElement] = []
func walk(_ e: AXUIElement, _ depth: Int) {
    guard depth < 40 && items.count < 3000 else { return }
    items.append(e)
    for child in value(e, kAXChildrenAttribute) as? [AXUIElement] ?? [] { walk(child, depth + 1) }
}
walk(app, 0)
let label: (AXUIElement) -> String = { e in
    let title = string(e, kAXTitleAttribute)
    return title.isEmpty ? string(e, kAXDescriptionAttribute) : title
}
if args[2] == "list" {
    for e in items {
        let role = string(e, kAXRoleAttribute)
        if ["AXButton", "AXCheckBox", "AXRadioButton", "AXTextField", "AXStaticText", "AXWindow"].contains(role) {
            // Never print text-field values, even for a synthetic credential.
            let text = role == "AXStaticText" ? string(e, kAXValueAttribute) : label(e)
            print("\(role): \(text)")
        }
    }
} else {
    guard args[2] == "click", args.count >= 4 else { fatalError("click label required") }
    let matches = items.filter { label($0) == args[3] && ["AXButton", "AXCheckBox", "AXRadioButton"].contains(string($0, kAXRoleAttribute)) }
    let index = args.count == 5 ? Int(args[4]) : nil
    guard (index != nil || matches.count == 1), let target = index.flatMap({ matches.indices.contains($0) ? matches[$0] : nil }) ?? (matches.count == 1 ? matches.first : nil) else { fatalError("label is absent or ambiguous") }
    let result = AXUIElementPerformAction(target, kAXPressAction as CFString)
    guard result == .success else { fatalError("AX action failed: \(result.rawValue)") }
    print("done")
}
