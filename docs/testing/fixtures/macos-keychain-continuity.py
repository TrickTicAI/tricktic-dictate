#!/usr/bin/env python3
"""Private, network-denied .37 -> .39 Keychain acceptance; never a release tool.

Run prepare/launch/show-keys/upgrade/cleanup from the logged-in Mac Terminal.
Observe the real keys-page status and password prompts before/after upgrade;
the helper never substitutes those observations or reads a key value back.
Artifacts, baseline path maps and logs stay in the ignored fixture directory.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import posixpath
import plistlib
import re
import subprocess
import sys
import tarfile

OLD_ARCHIVE = "80f34f2dc1c07298bf57cbebc27fab47aed5652917e21e139900eef2b91a89c3"
NEW_ARCHIVE = "71da573903bc9c5304a02270588230310e2db15b85a19a35525468504f3bd15c"
EXECUTABLES = {
    "0.37.0": "03a401fe3dbec711ac3761bf7ff3e7aee155495d682a39ad428e5489f6c36ab5",
    "0.39.0": "167e8403edd941b020e003dbce0ab4a1b94c5d049fbd5bcaab383508280385cb",
}
INSTALLER_TEST = "68cb56aba6a5d8e0b9835c717f2e528efd28365b1d424013f343fb3985ed4fd2"
LABEL = "mw-sec-29-private-keychain-continuity"
SERVICE = "com.metrix.writer"
DUMMY = "mw-sec-29-synthetic-not-a-real-api-key"
MARKER = "MW-SEC-22 disposable installer\n"


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def run(args, allowed=(0,), env=None):
    result = subprocess.run(args, capture_output=True, env=env)
    if result.returncode not in allowed:
        # Never echo command arguments, raw output, key material or UI contents.
        raise RuntimeError("{} failed ({})".format(Path(args[0]).name, result.returncode))
    return result


def guarded_root(repo, root):
    root = root.resolve()
    relative = root.relative_to(repo.resolve()).parts
    if len(relative) != 3 or relative[0] != ".local" or not relative[1].startswith("native-") or relative[2] != "same-identity":
        raise ValueError("Requires this worktree's .local/native-*/same-identity")
    return root


def receipt(root, name, data):
    path = root / (name + ".json")
    with path.open("x") as stream:
        json.dump(data, stream, indent=2)
        stream.write("\n")
    path.chmod(0o600)
    print(json.dumps(data))


def inventory(root):
    return {str(p.relative_to(root)): digest(p) for p in sorted(root.rglob("*")) if p.is_file()}


def identity(root, version):
    app = root / "install/TrickTic Dictate.app"
    exe = app / "Contents/MacOS/metrix-writer-desktop"
    if version not in EXECUTABLES or digest(exe) != EXECUTABLES[version]:
        raise ValueError("Not the retained exact signed executable")
    run(["codesign", "--verify", "--deep", "--strict", str(app)])
    text = run(["codesign", "-dv", "--verbose=4", str(app)]).stderr.decode()
    # The expected publisher team is the parenthesised suffix of the release
    # certificate name, e.g. "Developer ID Application: Name (TEAMID1234)".
    team = re.search(r"\(([A-Z0-9]{10})\)$", os.environ.get("WRITER_APPLE_DEVELOPER_ID", ""))
    if not team:
        raise ValueError("Set WRITER_APPLE_DEVELOPER_ID to the release Developer ID certificate name")
    if "Identifier=com.metrix.writer\n" not in text or "TeamIdentifier={}\n".format(team.group(1)) not in text:
        raise ValueError("Unexpected publisher or bundle identity")
    return exe


def owner_pid(root):
    job = run(["launchctl", "print", "gui/{}/{}".format(os.getuid(), LABEL)])
    match = re.search(rb"^\s*pid = (\d+)", job.stdout, re.M)
    if not match:
        raise RuntimeError("Private job has no running process")
    pid = int(match.group(1))
    command = run(["ps", "-p", str(pid), "-o", "command="]).stdout.decode()
    if str(root / "install/TrickTic Dictate.app/Contents/MacOS/metrix-writer-desktop") not in command or str(root / "profile") not in command:
        raise ValueError("Refusing unrelated process")
    return pid


def stop(root):
    domain = "gui/{}/{}".format(os.getuid(), LABEL)
    found = subprocess.run(["launchctl", "print", domain], capture_output=True)
    if found.returncode:
        return None
    pid = owner_pid(root)
    for path in (root / "profile/Data/native-recordings").glob("*/recording.json"):
        if json.loads(path.read_text()).get("capture_state") != "complete":
            raise ValueError("Stop and save private capture before cleanup/update")
    run(["launchctl", "bootout", domain])
    return pid


def key_item(root, delete=False):
    args = ["security", "delete-generic-password" if delete else "find-generic-password", "-s", SERVICE, "-a", str(root / "profile") + ":openai"]
    return run(args, allowed=(0, 44))


def prepare(args, repo, root):
    old = args.old_archive.resolve(strict=True)
    source = args.profile_source.resolve(strict=True)
    guarded_root(repo, source.parent)
    if source.name != "profile" or digest(old) != OLD_ARCHIVE:
        raise ValueError("Requires isolated profile and exact retained .37 archive")
    for p in source.rglob("*"):
        if p.is_symlink() and not p.resolve().is_relative_to(source):
            raise ValueError("Profile link escapes isolated source")
    with tarfile.open(old, "r:gz") as archive:
        for member in archive:
            p = Path(member.name)
            if p.is_absolute() or ".." in p.parts or not p.parts or p.parts[0] not in ["TrickTic Dictate.app", "._TrickTic Dictate.app"] or member.isdev() or member.isfifo():
                raise ValueError("Archive path outside expected private bundle")
            if member.issym() or member.islnk():
                target = Path(member.linkname)
                resolved = posixpath.normpath(str(p.parent / target) if member.issym() else str(target))
                if target.is_absolute() or not (resolved == "TrickTic Dictate.app" or resolved.startswith("TrickTic Dictate.app/")):
                    raise ValueError("Archive link escapes private bundle")
    root.mkdir(parents=True, exist_ok=False)
    (root / "install").mkdir()
    run(["tar", "-xzf", str(old), "-C", str(root / "install")])
    run(["cp", "-cR", str(source), str(root / "profile")])
    if list((root / "profile/State/credentials").glob("*.cred")):
        raise ValueError("Fixture must not contain provider credential files")
    prefs = root / "profile/State/tauri-preferences.json"
    if prefs.exists():
        (root / "original-private-preferences.json").write_bytes(prefs.read_bytes())
        data = json.loads(prefs.read_text())
        data["recording"] = "Ctrl+Alt+R"
        prefs.write_text(json.dumps(data) + "\n")
    else:
        (root / "profile/State/shortcuts.json").write_text(json.dumps({"Recording": "Ctrl+Alt+R", "Cancel": "Ctrl+Shift+F10", "Mode": "Hold"}) + "\n")
    (root / ".fixture").write_text(MARKER)
    (root / "home").mkdir()
    policy = '(version 1)\n(allow default)\n(deny network-outbound)\n'
    for directory in ["WebKit", "Caches"]:
        path = str(Path.home() / "Library" / directory / "com.metrix.writer")
        policy += '(deny file-read* file-write* (subpath {}))\n'.format(json.dumps(path))
    (root / "boundary.sb").write_text(policy)
    probe = "import socket,errno; s=socket.socket(); s.settimeout(2)\ntry: s.connect(('192.0.2.1',443))\nexcept PermissionError as e: assert e.errno==errno.EPERM; print('network-denied-EPERM')\nelse: raise RuntimeError('Outbound denial missing')"
    network = run(["sandbox-exec", "-f", str(root / "boundary.sb"), "/usr/bin/python3", "-c", probe])
    if network.stdout.strip() != b"network-denied-EPERM":
        raise RuntimeError("Outbound denial not established")
    exe = identity(root, "0.37.0")
    if key_item(root).returncode != 44:
        raise ValueError("Refusing pre-existing Keychain account")
    receipt(root, "item-ownership", {"service": SERVICE, "account": str(root / "profile") + ":openai", "preexisting_find_exit": 44, "creation_intent": "one synthetic task item; permits targeted partial-preparation cleanup"})
    # This is a public synthetic fixture value, never a user's provider key.
    run(["security", "add-generic-password", "-s", SERVICE, "-a", str(root / "profile") + ":openai", "-w", DUMMY, "-T", str(exe)])
    if key_item(root).returncode != 0:
        raise RuntimeError("Created task item not found")
    receipt(root, "prepared", {"at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "old_version": "0.37.0", "old_archive_sha256": OLD_ARCHIVE, "old_executable_sha256": EXECUTABLES["0.37.0"], "outbound_denied_EPERM": True, "service": SERVICE, "account": str(root / "profile") + ":openai", "synthetic_item_created": True, "matched_or_key_values_retained": False, "fixture_sha256": digest(Path(__file__)), "native_ui_read": "pending human observation"})


def launch(args, root):
    exe = identity(root, args.version)
    if (root / ("launch-" + args.version + ".json")).exists():
        raise ValueError("Preserve earlier launch receipt; use a new fixture for another attempt")
    domain = "gui/{}/{}".format(os.getuid(), LABEL)
    if subprocess.run(["launchctl", "print", domain], capture_output=True).returncode == 0:
        raise ValueError("Private job already exists")
    command = ["/usr/bin/env"]
    for name in ["OPENAI_API_KEY", "GEMINI_API_KEY", "XAI_API_KEY", "OPENROUTER_API_KEY", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH", "DYLD_FRAMEWORK_PATH"]:
        command += ["-u", name]
    command += ["CFFIXED_USER_HOME=" + str(root / "home"), "__XPC_CFFIXED_USER_HOME=" + str(root / "home"), "/usr/bin/sandbox-exec", "-f", str(root / "boundary.sb"), str(exe), "--user-root", str(root / "profile")]
    # Explicit one-shot job: startup failures must not create a relaunch loop.
    plist = root / ("launch-" + args.version + ".plist")
    with plist.open("xb") as stream:
        plistlib.dump({"Label": LABEL, "ProgramArguments": command,
                      "RunAtLoad": True, "KeepAlive": False,
                      "StandardOutPath": str(root / (args.version + ".log")),
                      "StandardErrorPath": str(root / (args.version + ".error.log"))}, stream)
    plist.chmod(0o600)
    run(["launchctl", "bootstrap", "gui/{}".format(os.getuid()), str(plist)])
    receipt(root, "launch-" + args.version, {"version": args.version, "executable_sha256": EXECUTABLES[args.version], "network_outbound_denied": True, "label": LABEL, "candidate": args.candidate, "keep_alive": False, "result": "bootstrapped; verify PID and observe UI separately"})


def show_keys(root):
    pid = owner_pid(root)
    # The real UI, not injected JS or a replacement response; no key is typed.
    script = '''tell application "System Events"
set p to first process whose unix id is PID
set frontmost of p to true
repeat with w in windows of p
try
set value of attribute "AXMinimized" of w to false
perform action "AXRaise" of w
end try
end repeat
delay 0.4
if (unix id of (first process whose frontmost is true)) is not PID then error "Focus changed before input"
key code 53
keystroke "k" using command down
delay 0.4
if (unix id of (first process whose frontmost is true)) is not PID then error "Focus changed"
keystroke "API keys"
delay 0.4
key code 36
end tell'''.replace("PID", str(pid))
    run(["osascript", "-e", script])
    print(json.dumps({"verified_private_pid": pid, "navigation_requested": "real API keys screen", "ui_status_and_prompt_observation": "pending human answer"}))


def upgrade(args, root):
    archive = args.new_archive.resolve(strict=True)
    test = args.installer_test.resolve(strict=True)
    if digest(archive) != NEW_ARCHIVE or digest(test) != INSTALLER_TEST:
        raise ValueError("Requires retained exact candidate and native installer fixture")
    identity(root, "0.37.0")
    stop(root)
    before = inventory(root / "profile")
    env = dict(os.environ)
    for name in ["DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH", "DYLD_FRAMEWORK_PATH"]:
        env.pop(name, None)
    env.update({"WRITER_INSTALL_FIXTURE_ROOT": str(root), "WRITER_INSTALL_FIXTURE_ARCHIVE": str(archive)})
    result = run([str(test), "--ignored", "--exact", "retained_signed_archive_installs_in_private_copy", "--nocapture", "--test-threads=1"], env=env)
    identity(root, "0.39.0")
    after = inventory(root / "profile")
    if before != after or key_item(root).returncode != 0:
        raise RuntimeError("Saved data or task item not preserved")
    receipt(root, "private-upgrade", {"installer_exit": result.returncode, "new_archive_sha256": NEW_ARCHIVE, "old_executable_sha256": EXECUTABLES["0.37.0"], "new_executable_sha256": EXECUTABLES["0.39.0"], "profile_files": len(before), "profile_identical_across_install": True, "task_keychain_item_exists": True, "post_update_native_ui_read": "pending human observation", "full_in_app_update_chain": False})


def cleanup(root):
    prepared = json.loads((root / "item-ownership.json").read_text())
    if prepared["preexisting_find_exit"] != 44 or prepared["service"] != SERVICE or prepared["account"] != str(root / "profile") + ":openai":
        raise ValueError("Refusing unknown Keychain item")
    pid = stop(root)
    deleted = key_item(root, delete=True)
    absent = key_item(root)
    if absent.returncode != 44:
        raise RuntimeError("Task item still exists")
    receipt(root, "cleanup", {"stopped_owned_pid": pid, "only_task_item_targeted": True, "delete_exit": deleted.returncode, "find_after_exit": absent.returncode, "key_values_read_back": False, "permissions_modified": False, "usual_preferences_modified": False})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["prepare", "launch", "show-keys", "upgrade", "cleanup"])
    parser.add_argument("--candidate", choices=["historical", "remediated"], default="historical")
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--old-archive", type=Path)
    parser.add_argument("--new-archive", type=Path)
    parser.add_argument("--profile-source", type=Path)
    parser.add_argument("--installer-test", type=Path)
    parser.add_argument("--version", choices=list(EXECUTABLES))
    args = parser.parse_args()
    global NEW_ARCHIVE, INSTALLER_TEST, LABEL
    if args.candidate == "remediated":
        NEW_ARCHIVE = "496b1d27ccc47447a5e3e66ca0924aafcca173777f3f84279dc22f6513a79983"
        EXECUTABLES["0.39.0"] = "0607e4bc967bbfc83fd4a0a730b61feb4d8b26700c88ec053366a736c4986cba"
        INSTALLER_TEST = "32bcd619aa859d89cd114322fb8168efa450e3c2a3225f4cfb79fd8368a37ad5"
        LABEL += "-remediated"
    if sys.platform != "darwin":
        parser.error("Native macOS fixture only")
    repo = Path(__file__).resolve().parents[3]
    root = guarded_root(repo, args.root)
    if args.mode == "prepare":
        if args.old_archive is None or args.profile_source is None:
            parser.error("prepare needs --old-archive and --profile-source")
        prepare(args, repo, root)
        return
    if (root / ".fixture").read_text() != MARKER:
        raise ValueError("Private installer marker missing")
    if args.mode == "launch":
        if args.version is None:
            parser.error("launch needs --version")
        launch(args, root)
    elif args.mode == "show-keys":
        show_keys(root)
    elif args.mode == "upgrade":
        if args.new_archive is None or args.installer_test is None:
            parser.error("upgrade needs --new-archive and --installer-test")
        upgrade(args, root)
    else:
        cleanup(root)


if __name__ == "__main__":
    main()
