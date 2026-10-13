#!/usr/bin/env python3
"""Bounded pre-authentication probes against a scratch Mac Whisper child.

Uses only its own process and loopback listener, an existing model read-only,
at most 8 MiB in one unfinished header and 16 idle connections. No audio,
inference, installed process, persistent credential or real user data is used.
"""
import argparse
import datetime
import hashlib
import http.client
import json
from pathlib import Path
import queue
import re
import secrets
import socket
import subprocess
import threading
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    worker = args.worker.resolve(strict=True)
    assert "/.local/native-" in str(worker) and "/runtime/whisper/cpu/whisper-server" in str(worker)
    assert not args.out.exists(), "Preserve earlier receipts"
    capability, instance = secrets.token_hex(32), secrets.token_hex(32)
    child = subprocess.Popen([str(worker), "-m", str(args.model.resolve(strict=True)), "-ng", "-nt", "-t", "2", "--port", "0"], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    lines = queue.Queue()
    leaked = []

    def drain():
        for line in iter(child.stderr.readline, b""):
            if capability.encode() in line or instance.encode() in line:
                leaked.append(True)
            found = re.search(rb"writer: private listener (\d+)", line)
            if found:
                lines.put(int(found[1]))

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    held = []
    result = {"schema": 1, "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "worker_sha256": hashlib.sha256(worker.read_bytes()).hexdigest(), "checks": {}}
    try:
        child.stdin.write(f"{capability}\n{instance}\n".encode())
        child.stdin.close()
        port = lines.get(timeout=120)

        def request(auth=True):
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
            started = time.monotonic()
            try:
                connection.request("GET", "/health", headers={"Authorization": "Bearer " + capability} if auth else {})
                response = connection.getresponse()
                response.read(4096)
                return {"status": response.status, "seconds": round(time.monotonic() - started, 3)}
            except (OSError, http.client.HTTPException) as error:
                return {"error": type(error).__name__, "seconds": round(time.monotonic() - started, 3)}
            finally:
                connection.close()

        deadline = time.monotonic() + 120
        while request().get("status") != 200:
            assert child.poll() is None and time.monotonic() < deadline, "Worker readiness failed"
            time.sleep(.2)
        listeners = subprocess.check_output(["/usr/sbin/lsof", "-nP", "-a", "-p", str(child.pid), "-iTCP", "-sTCP:LISTEN", "-Fn"], text=True)
        addresses = [line[1:] for line in listeners.splitlines() if line.startswith("n")]
        assert addresses == [f"127.0.0.1:{port}"]
        checks = result["checks"]
        checks["loopback_only"] = True
        checks["owner_health"] = request()
        checks["unauthenticated_health"] = request(False)

        def rss():
            return int(subprocess.check_output(["ps", "-p", str(child.pid), "-o", "rss="], text=True).strip()) * 1024

        baseline = peak = rss()
        sent = 0
        refused = None
        with socket.create_connection(("127.0.0.1", port), timeout=2) as connection:
            connection.sendall(b"GET /health HTTP/1.1\r\nHost: localhost\r\nX-Probe: ")
            for _ in range(8):
                try:
                    connection.sendall(b"a" * (1 << 20))
                    sent += 1 << 20
                    peak = max(peak, rss())
                except OSError as error:
                    refused = type(error).__name__
                    break
        checks["unterminated_header"] = {"bytes_sent": sent, "refused": refused, "rss_baseline": baseline, "rss_peak": peak, "owner_health_after": request()}
        for _ in range(16):
            try:
                connection = socket.create_connection(("127.0.0.1", port), timeout=2)
                held.append(connection)
                connection.sendall(b"GET /health HTTP/1.1\r\n")
            except OSError:
                break
        checks["idle_connections"] = {"opened": len(held), "owner_health_while_held": request()}
        for connection in held:
            connection.close()
        held.clear()
        checks["owner_health_after_release"] = request()
        result["worker_alive_at_end"] = child.poll() is None
    finally:
        for connection in held:
            connection.close()
        child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
        reader.join(timeout=2)
    assert not leaked, "Worker logged its synthetic capability"
    result["owned_child_stopped"] = True
    result["capability_log_leak"] = False
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
