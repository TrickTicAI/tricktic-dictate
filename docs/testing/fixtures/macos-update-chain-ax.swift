// Read/press only fixed update controls of an exact private GUI-chain process.
import AppKit
import ApplicationServices
import CryptoKit
import Foundation

let args = CommandLine.arguments
guard args.count >= 4, let pid = Int32(args[2]) else { fatalError("root pid inspect|press|press-settings|focus|enhance|capture [label/phase]") }
let root = URL(fileURLWithPath: args[1]).standardizedFileURL
let prefix = FileManager.default.currentDirectoryPath + "/.local/native-20261012-gui-chain/"
let keyRoot = FileManager.default.currentDirectoryPath + "/.local/native-20261013/same-identity"
let chainRoot = root.path.hasPrefix(prefix) && ["legacy", "same-identity"].contains(root.lastPathComponent)
var keyFixture = false
if root.path == keyRoot {
    let item = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("item-ownership.json"))) as! [String: Any]
    guard item["service"] as? String == "com.metrix.writer",
          item["account"] as? String == root.path + "/profile:openai",
          item["preexisting_find_exit"] as? Int == 44 else { fatalError("Unknown task Keychain item") }
    keyFixture = true
}
guard chainRoot || keyFixture,
      let running = NSRunningApplication(processIdentifier: pid),
      let executable = running.executableURL,
      executable.path.hasPrefix(root.path + "/install/"),
      let appURL = running.bundleURL,
      let bundle = Bundle(url: appURL),
      let version = bundle.infoDictionary?["CFBundleShortVersionString"] as? String,
      bundle.bundleIdentifier == "com.metrix.writer" else { fatalError("Refusing unrelated app") }
let expected: [String: [String: Any]]
if keyFixture {
    expected = ["0.37.0": ["executable_sha256": "03a401fe3dbec711ac3761bf7ff3e7aee155495d682a39ad428e5489f6c36ab5"],
                "0.39.0": ["executable_sha256": "0607e4bc967bbfc83fd4a0a730b61feb4d8b26700c88ec053366a736c4986cba"]]
} else {
    expected = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("expected-identities.json"))) as! [String: [String: Any]]
}
let sha = SHA256.hash(data: try Data(contentsOf: executable)).map { String(format: "%02x", $0) }.joined()
guard expected[version]?["executable_sha256"] as? String == sha else { fatalError("Private executable identity changed") }
let app = AXUIElementCreateApplication(pid)
if args[3] == "quit" {
    guard args.count == 4 else { fatalError("Unexpected quit arguments") }
    let recordings = root.appendingPathComponent("profile/Data/native-recordings")
    if let files = FileManager.default.enumerator(at: recordings, includingPropertiesForKeys: nil) {
        for case let file as URL in files where file.lastPathComponent == "recording.json" {
            let data = try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as! [String: Any]
            guard data["capture_state"] as? String == "complete" else { fatalError("Save private recording before quit") }
        }
    }
    guard running.terminate() else { fatalError("Private graceful quit refused") }
    print("Requested quit only for verified private app")
    exit(0)
}
if args[3] == "capture" {
    guard args.count == 5, args[4].allSatisfy({ $0.isLetter || $0.isNumber || $0 == "-" }),
          !args[4].isEmpty else { fatalError("Invalid private capture phase") }
    // Read permission state only. Never request another app's TCC grant.
    guard CGPreflightScreenCaptureAccess() else {
        print("Private window capture unavailable; no screen-capture permission request made")
        exit(3)
    }
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    let owned = windows.filter {
        ($0[kCGWindowOwnerPID as String] as? Int32) == pid &&
        ($0[kCGWindowLayer as String] as? Int) == 0
    }
    guard owned.count == 1, let id = owned[0][kCGWindowNumber as String] as? Int else {
        fatalError("Private on-screen window absent or ambiguous")
    }
    let output = root.appendingPathComponent(args[4] + ".png")
    guard !FileManager.default.fileExists(atPath: output.path) else { fatalError("Capture already exists") }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    process.arguments = ["-x", "-l", String(id), output.path]
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else { fatalError("Private capture unavailable") }
    try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: output.path)
    print("Captured only the verified private app window")
    exit(0)
}
func value(_ e: AXUIElement, _ name: String) -> CFTypeRef? {
    var v: CFTypeRef?
    return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}
func string(_ e: AXUIElement, _ name: String) -> String { value(e, name) as? String ?? "" }
let allowed: Set<String> = ["API keys", "OpenAI", "Key stored in your macOS login Keychain", "Settings", "About", "About & updates", "Check for updates", "Download 0.35.0", "Download 0.39.0", "Update and restart", "Cancel download", "Ready to install", "Writer is up to date", "Update failed", "Could not check for updates", "The update feed replayed or replaced a previously verified release.", "Version 0.35.0 is available", "Version 0.39.0 is available"]
if args[3] == "focus" {
    running.activate(options: [.activateIgnoringOtherApps])
    for w in value(app, kAXWindowsAttribute) as? [AXUIElement] ?? [] {
        AXUIElementSetAttributeValue(w, kAXMinimizedAttribute as CFString, kCFBooleanFalse)
        AXUIElementPerformAction(w, kAXRaiseAction as CFString)
    }
    print("Private verified app raised")
    exit(0)
}
if args[3] == "enhance" {
    // Ask only this private process for its standard accessibility UI tree.
    for attribute in ["AXManualAccessibility", "AXEnhancedUserInterface"] {
        let result = AXUIElementSetAttributeValue(app, attribute as CFString, kCFBooleanTrue)
        print("Private \(attribute) result: \(result.rawValue)")
    }
    exit(0)
}
var items: [AXUIElement] = []
func walk(_ e: AXUIElement, _ depth: Int) {
    guard depth < 50 && items.count < 5000 else { return }
    items.append(e)
    for child in value(e, kAXChildrenAttribute) as? [AXUIElement] ?? [] { walk(child, depth + 1) }
}
walk(app, 0)
func label(_ e: AXUIElement) -> String {
    let t = string(e, kAXTitleAttribute)
    if !t.isEmpty { return t }
    let d = string(e, kAXDescriptionAttribute)
    return d.isEmpty ? string(e, kAXValueAttribute) : d
}
if args[3] == "press-settings" {
    guard args.count == 4, AXIsProcessTrusted() else { fatalError("Native menu access unavailable") }
    let names: Set<String> = ["Settings…", "Settings..."]
    let matches = items.filter { names.contains(label($0)) && string($0, kAXRoleAttribute) == "AXMenuItem" }
    guard matches.count == 1, value(matches[0], kAXEnabledAttribute) as? Bool == true else {
        fatalError("Private Settings menu absent, disabled or ambiguous")
    }
    guard AXUIElementPerformAction(matches[0], kAXPressAction as CFString) == .success else {
        fatalError("Private Settings menu action failed")
    }
    print("Pressed verified private native Settings menu")
} else if args[3] == "inspect" {
    // Output only allowlisted labels, never arbitrary app text or field values.
    let labels = items.flatMap { e -> [String] in
        let text = [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute, kAXHelpAttribute]
            .map { string(e, $0) }
        return allowed.filter { l in text.contains { $0.contains(l) } }.sorted()
    }
    let roles = Dictionary(grouping: items, by: { string($0, kAXRoleAttribute) }).mapValues { $0.count }
    let result: [String: Any] = ["pid": pid, "version": version, "elements": items.count,
                               "known_labels": Array(Set(labels)).sorted(), "role_counts": roles,
                               "caller_ax_trusted": AXIsProcessTrusted()]
    print(String(data: try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]), encoding: .utf8)!)
} else {
    guard args[3] == "press", args.count == 5, allowed.contains(args[4]) else { fatalError("Control outside update allowlist") }
    let matches = items.filter { label($0) == args[4] && string($0, kAXRoleAttribute) == "AXButton" }
    guard matches.count == 1 else { fatalError("Update control absent or ambiguous") }
    guard AXUIElementPerformAction(matches[0], kAXPressAction as CFString) == .success else { fatalError("Private UI action failed") }
    print("Pressed verified private update control")
}
