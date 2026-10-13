#!/usr/bin/env python3
"""Exercise a patched speech worker using synthetic silence; owns and stops both launches.
Usage: worker-auth-smoke.py --family whisper -- /path/whisper-server -m /path/model -ng
Also accepts the compiled auth.test.cpp fixture (append nemo for the NeMo fixture).
No credentials, audio or transcripts are written; stdout contains only a test receipt.
"""
import argparse
import http.client
import io
import json
from pathlib import Path
import queue
import re
import secrets
import subprocess
import sys
import threading
import time
import wave


def silence():
    output = io.BytesIO()
    with wave.open(output, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16000)
        wav.writeframes(b"\0\0" * 16000)
    return output.getvalue()


def run(command, family, old_capability=None, backend=None):
    capability, instance = secrets.token_hex(32), secrets.token_hex(32)
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE,
                             creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    messages, logs = queue.Queue(), []
    def drain(pipe, report=False):
        for line in iter(pipe.readline, b""):
            logs.append(line)
            if report:
                messages.put(line)
    readers = [threading.Thread(target=drain, args=(child.stderr, True), daemon=True),
               threading.Thread(target=drain, args=(child.stdout,), daemon=True)]
    for reader in readers:
        reader.start()
    try:
        child.stdin.write((capability + "\n" + instance + "\n").encode())
        child.stdin.close()
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if child.poll() is not None:
                raise RuntimeError("Worker exited before private readiness")
            try:
                line = messages.get(timeout=.2)
            except queue.Empty:
                continue
            if line.startswith(b"writer: private listener "):
                port = int(line.split()[-1])
                break
        else:
            raise RuntimeError("Private listener handshake timed out")

        def request(path, method="GET", body=None, auth=capability, headers=None):
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
            sent = dict(headers or {})
            if auth is not None:
                sent["Authorization"] = "Bearer " + auth
            try:
                connection.request(method, path, body=body, headers=sent)
                response = connection.getresponse()
                payload = response.read(65536)
                return response.status, response.getheader("X-Writer-Instance"), payload
            finally:
                connection.close()

        health = "/ready" if family == "nemo" else "/health"
        code, identity, body = request(health)
        assert code == 200 and identity == instance
        assert json.loads(body).get("ready" if family == "nemo" else "warm") is True
        inference = "/v1/audio/transcriptions" if family == "nemo" else "/inference"
        # A second OS process receives only the public port/path, never a secret.
        subprocess.run([sys.executable, "-c", """
import http.client, sys
for path, method in [(sys.argv[2], 'GET'), (sys.argv[3], 'POST'), ('/writer/finalize', 'POST')]:
    c = http.client.HTTPConnection('127.0.0.1', int(sys.argv[1]), timeout=5)
    c.request(method, path)
    r = c.getresponse()
    assert r.status == 401
    r.read()
    c.close()
""", str(port), health, inference], check=True, capture_output=True)
        duplicate = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            duplicate.putrequest("GET", health)
            duplicate.putheader("Authorization", "Bearer " + capability)
            duplicate.putheader("Authorization", "Bearer " + capability)
            duplicate.endheaders()
            denied = duplicate.getresponse()
            assert denied.status == 401
            denied.read()
        finally:
            duplicate.close()

        for path, method in [(health, "GET"), (inference, "POST"),
                             ("/writer/finalize", "POST"), ("/load", "POST"),
                             ("/v1/realtime", "GET")]:
            assert request(path, method, auth=None)[0] == 401
            assert request(path, method, auth="wrong")[0] == 401
            assert request(path, method, headers={"Origin": "https://untrusted.invalid"})[0] == 403
            assert request(path, method, headers={"Origin": "null"})[0] == 403
            assert request(path, method, headers={"Sec-Fetch-Site": "cross-site"})[0] == 403
        assert request(inference, "OPTIONS", headers={"Origin": "https://untrusted.invalid"})[0] == 403
        assert request("/load", "POST")[0] == 404
        assert request("/v1/realtime", headers={"Upgrade": "websocket", "Connection": "Upgrade",
                                              "Sec-WebSocket-Version": "13",
                                              "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ=="})[0] == 404  # gitleaks:allow
        assert request(health + "?api_key=query-only", auth=None)[0] == 401
        assert request(health + "?api_key=unused")[0] == 404
        if old_capability:
            assert request(health, auth=old_capability)[0] == 401
        # Unauthorized bodies must be rejected before reading multipart/audio bytes.
        assert request(inference, "POST", auth=None, headers={"Content-Length": "1000000"})[0] == 401
        if family == "whisper":
            assert request("/writer/finalize", "POST", body=b"")[0] == 200
        boundary = "writer-synthetic-audio"
        body = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"silence.wav\"\r\n"
                "Content-Type: audio/wav\r\n\r\n").encode() + silence()
        body += (f"\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\n"
                 f"json\r\n--{boundary}--\r\n").encode()
        code, identity, _ = request(inference, "POST", body=body,
                                   headers={"Content-Type": "multipart/form-data; boundary=" + boundary})
        assert code == 200 and identity == instance, "Owner synthetic inference failed"
        proc = Path(f"/proc/{child.pid}")
        if proc.exists():
            for name in ["cmdline", "environ"]:
                content = (proc / name).read_bytes()
                assert capability.encode() not in content and instance.encode() not in content
    finally:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        for reader in readers:
            reader.join(timeout=2)
        content = b"".join(logs)
        assert capability.encode() not in content and instance.encode() not in content, "Handshake leaked"
    if backend:
        log = content.decode(errors="replace")
        if family == "nemo":
            marker = {"cpu": r"backend=CPU", "vulkan": r"backend=Vulkan", "metal": r"backend=MTL"}[backend]
        else:
            marker = {"cpu": r"use gpu\s*= 0", "vulkan": r"using Vulkan.*backend",
                      "metal": r"using MTL.*backend"}[backend]
        assert re.search(marker, log), "Requested backend was not proved by the child log"
    return capability


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--family", choices=["whisper", "nemo"], required=True)
    parser.add_argument("--expect-backend", choices=["cpu", "vulkan", "metal"])
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("Supply the worker command after --")
    first = run(command, args.family, backend=args.expect_backend)
    run(command, args.family, first, backend=args.expect_backend)
    print(json.dumps({"family": args.family, "backend": args.expect_backend, "launches": 2, "owner_synthetic_inference": "passed",
                      "unauthorized_browser_admin": "denied", "restart_rotation": "passed",
                      "capability_leak_check": "passed"}))


if __name__ == "__main__":
    main()
