#!/usr/bin/env python3
"""Runs the Android account-sync adapter against the real Firestore emulator (MW-OUATH-7).

Starts a private Cloud Firestore emulator on a free loopback port with the repository's
infra/firebase/firestore.rules and a demo- project, runs FirestoreEmulatorTest on an explicit
ready device (the Android emulator reaches the host as 10.0.2.2), then stops the emulator.
It never contacts or deploys to a real Firebase project.

    python3 scripts/account-sync-emulator.py --serial emulator-5640

Needs JAVA_HOME (JDK 21+), the installed dev debug app and androidTest APKs, and the emulator
JAR that `firebase` downloads (~/.cache/firebase/emulators). Pass --jar to choose another.
"""
import argparse
import glob
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
PROJECT = "demo-metrix-writer"
TEST = "com.metrix.writer.FirestoreEmulatorTest"
RUNNER = "com.metrix.writer.debug.test/android.test.InstrumentationTestRunner"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--serial", required=True, help="ready emulator serial, e.g. emulator-5640")
    parser.add_argument("--jar", help="cloud-firestore-emulator JAR (default: newest in ~/.cache/firebase/emulators)")
    args = parser.parse_args()
    if not args.serial.startswith("emulator-"):
        sys.exit("Use an Android emulator serial: the test reaches the host as 10.0.2.2")
    jars = sorted(glob.glob(os.path.expanduser("~/.cache/firebase/emulators/cloud-firestore-emulator-v*.jar")))
    jar = args.jar or (jars[-1] if jars else None)
    if not jar or not os.path.isfile(jar):
        sys.exit("No Firestore emulator JAR. Run `npm ci --prefix infra/firebase/emulator` and the rules suite once, or pass --jar")
    java = os.path.join(os.environ["JAVA_HOME"], "bin", "java") if os.environ.get("JAVA_HOME") else shutil.which("java")
    adb = shutil.which("adb") or os.path.join(os.environ.get("ANDROID_HOME", ""), "platform-tools", "adb")
    port, websocket = free_port(), free_port()
    work = tempfile.mkdtemp(prefix="writer-android-firestore-")
    log = open(os.path.join(work, "emulator.log"), "w")
    emulator = subprocess.Popen([java, "-jar", jar, "--host", "127.0.0.1", "--port", str(port), "--websocket_port", str(websocket),
                                 "--project_id", PROJECT, "--single_project_mode", "true",
                                 "--rules", os.path.join(ROOT, "infra", "firebase", "firestore.rules")],
                                cwd=work, stdout=log, stderr=subprocess.STDOUT)
    try:
        deadline = time.time() + 60
        while True:
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=2).read()
                break
            except OSError:
                if emulator.poll() is not None or time.time() > deadline:
                    sys.exit(f"Firestore emulator did not start; see {log.name}")
                time.sleep(0.5)
        result = subprocess.run([adb, "-s", args.serial, "shell", "am", "instrument", "-w", "-e", "class", TEST,
                                 "-e", "firestoreEmulator", f"10.0.2.2:{port}", RUNNER], capture_output=True, text=True, timeout=900)
        print(result.stdout.strip())
        passed = "OK (" in result.stdout and "FAILURES" not in result.stdout
        print(f"Firestore emulator {os.path.basename(jar)} on 127.0.0.1:{port}, project {PROJECT}: {'passed' if passed else 'FAILED'}")
        return 0 if passed else 1
    finally:
        emulator.terminate()
        try:
            emulator.wait(timeout=20)
        except subprocess.TimeoutExpired:
            emulator.kill()
        log.close()
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
