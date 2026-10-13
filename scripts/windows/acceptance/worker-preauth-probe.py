#!/usr/bin/env python3
"""MW-SEC-21: measure what an unauthenticated local client can do to a private worker.

Run with native Windows Python against a scratch whisper-server that this script
launches and stops itself (private handshake on stdin, synthetic requests only):
  worker-preauth-probe.py --out receipt.json -- <whisper-server.exe> -m <model> -ng
Bounded: at most --header-mib of header bytes and --hold idle connections.
It never touches an installed Writer process, its port or its data.
"""
import argparse
import ctypes
import http.client
import json
import re
import secrets
import socket
import subprocess
import sys
import threading
import time


class Counters(ctypes.Structure):
    _fields_ = [('cb', ctypes.c_uint32), ('PageFaultCount', ctypes.c_uint32)] + [
        (name, ctypes.c_size_t) for name in ('PeakWorkingSetSize', 'WorkingSetSize', 'QuotaPeakPagedPoolUsage',
                                             'QuotaPagedPoolUsage', 'QuotaPeakNonPagedPoolUsage',
                                             'QuotaNonPagedPoolUsage', 'PagefileUsage', 'PeakPagefileUsage')]


def private_bytes(process):
    if sys.platform != 'win32':
        # Linux iteration aid: anonymous resident memory approximates private bytes.
        with open(f'/proc/{process.pid}/status', encoding='ascii') as status:
            fields = dict(line.split(':', 1) for line in status if ':' in line)
        return int(fields['RssAnon'].split()[0]) * 1024
    counters = Counters(cb=ctypes.sizeof(Counters))
    handle = ctypes.windll.kernel32.OpenProcess(0x1000 | 0x0010, False, process.pid)
    try:
        if not ctypes.windll.psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            raise OSError('GetProcessMemoryInfo failed')
        return counters.PagefileUsage
    finally:
        ctypes.windll.kernel32.CloseHandle(handle)


def request(port, path='/health', headers=None, timeout=10):
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=timeout)
    started = time.monotonic()
    try:
        connection.request('GET', path, headers=headers or {})
        return connection.getresponse().status, round(time.monotonic() - started, 3)
    except (OSError, http.client.HTTPException) as error:
        return type(error).__name__, round(time.monotonic() - started, 3)
    finally:
        connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', required=True)
    parser.add_argument('--header-mib', type=int, default=256)
    parser.add_argument('--hold', type=int, default=64)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    capability, instance = secrets.token_hex(32), secrets.token_hex(32)
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                             creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    result = {'schema': 1, 'kind': 'mw-sec-21-worker-preauth', 'worker': command[0], 'checks': {}}
    try:
        child.stdin.write(f'{capability}\n{instance}\n'.encode())
        child.stdin.close()
        port, deadline = None, time.monotonic() + 120
        while port is None and time.monotonic() < deadline:
            line = child.stderr.readline().decode(errors='replace')
            if not line and child.poll() is not None:
                raise SystemExit('worker exited before listening')
            found = re.search(r'writer: private listener (\d+)', line)
            port = int(found.group(1)) if found else None
        threading.Thread(target=lambda: [None for _ in iter(child.stderr.readline, b'')], daemon=True).start()
        owner = {'Authorization': f'Bearer {capability}'}
        while request(port, headers=owner)[0] != 200 and time.monotonic() < deadline:
            time.sleep(0.5)
        checks = result['checks']
        checks['owner_health'] = request(port, headers=owner)
        checks['unauthenticated_health'] = request(port)
        checks['browser_origin_with_capability'] = request(port, headers={**owner, 'Origin': 'https://example.invalid'})

        # 1. One unterminated header line, sent without any credential.
        baseline = private_bytes(child)
        sock = socket.create_connection(('127.0.0.1', port), timeout=10)
        sent, peak, refused = 0, baseline, None
        try:
            sock.sendall(b'GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Probe: ')
            block = b'a' * (1 << 20)
            for _ in range(args.header_mib):
                sock.sendall(block)
                sent += len(block)
                peak = max(peak, private_bytes(child))
        except OSError as error:
            refused = type(error).__name__
        finally:
            sock.close()
        time.sleep(1)
        checks['unterminated_header'] = {'bytes_sent': sent, 'refused_with': refused,
                                         'private_bytes_baseline': baseline, 'private_bytes_peak': peak,
                                         'growth_per_sent_byte': round((peak - baseline) / max(sent, 1), 3),
                                         'worker_alive_after': child.poll() is None,
                                         'owner_health_after': request(port, headers=owner)}

        # 2. Idle unauthenticated connections that never finish their headers.
        held = []
        for _ in range(args.hold):
            try:
                s = socket.create_connection(('127.0.0.1', port), timeout=5)
                s.sendall(b'GET /health HTTP/1.1\r\n')
                held.append(s)
            except OSError:
                break
        time.sleep(0.5)
        checks['held_connections'] = {'opened': len(held), 'owner_health_while_held': request(port, headers=owner, timeout=30)}
        for s in held:
            s.close()
        checks['owner_health_after_release'] = request(port, headers=owner)
        result['worker_alive_at_end'] = child.poll() is None
    finally:
        child.kill()
        child.wait(timeout=30)
    with open(args.out, 'x', encoding='utf-8') as out:
        json.dump(result, out, indent=2)
    print(json.dumps(result['checks'], indent=1))


if __name__ == '__main__':
    main()
