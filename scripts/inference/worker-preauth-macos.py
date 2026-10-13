#!/usr/bin/env python3
"""Bounded native Mac acceptance of owned, packaged Whisper helpers.

Only launches/stops the supplied scratch worker. No installed process, saved
recording or credential is touched. Sends at most 8 MiB and holds at most 96
loopback sockets; idle deadlines are tested BEFORE closing those sockets.
Receipts contain hashes, counters/statuses and booleans, never handshake values,
model paths, helper output, audio or transcript text.
"""
import argparse
import datetime as dt
import hashlib
import http.client
import io
import json
from pathlib import Path
import queue
import re
import secrets
import select
import socket
import subprocess
import sys
import threading
import time
import wave


LIMITS_MARKER = b'writer: pre-auth limits line=8192 header=8192 count=64 read=10 threads=32 queue=32'


def counters(child):
    rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(child.pid)]).strip()) * 1024
    threads = len(subprocess.check_output(['ps', '-M', '-p', str(child.pid)]).splitlines()) - 1
    return {'rss_bytes': rss, 'threads': threads}


def run(command, backend):
    if sys.platform != 'darwin':
        raise RuntimeError('Native macOS execution required')
    worker = Path(command[0]).resolve()
    before = hashlib.sha256(worker.read_bytes()).hexdigest()
    capability, instance = secrets.token_hex(32), secrets.token_hex(32)
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    messages, logs = queue.Queue(), []
    def drain(pipe, report=False):
        for line in iter(pipe.readline, b''):
            logs.append(line)
            if report:
                messages.put(line)
    readers = [threading.Thread(target=drain, args=(child.stderr, True), daemon=True),
               threading.Thread(target=drain, args=(child.stdout,), daemon=True)]
    for reader in readers:
        reader.start()
    held = []
    result = {'schema': 1, 'kind': 'macos-packaged-whisper-preauth',
              'checked_at': dt.datetime.now(dt.timezone.utc).isoformat(), 'backend': backend,
              'worker_sha256': before, 'passed': False, 'checks': {}}
    responses = []
    try:
        child.stdin.write(f'{capability}\n{instance}\n'.encode())
        child.stdin.close()
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if child.poll() is not None:
                raise RuntimeError('Worker exited before readiness')
            try:
                line = messages.get(timeout=.2)
            except queue.Empty:
                continue
            if line.startswith(b'writer: private listener '):
                port = int(line.split()[-1])
                break
        else:
            raise RuntimeError('Private listener timeout')

        def request(auth=capability, extra=None, timeout=2, path='/health', method='GET', body=None):
            connection = http.client.HTTPConnection('127.0.0.1', port, timeout=timeout)
            started = time.monotonic()
            try:
                headers = dict(extra or {})
                if auth is not None:
                    headers['Authorization'] = 'Bearer ' + auth
                connection.request(method, path, body=body, headers=headers)
                response = connection.getresponse()
                payload = response.read(65536)
                responses.append(payload)
                return {'status': response.status, 'seconds': round(time.monotonic() - started, 3),
                        'owner_identity': response.getheader('X-Writer-Instance') == instance}
            except (OSError, http.client.HTTPException) as error:
                return {'error': type(error).__name__, 'seconds': round(time.monotonic() - started, 3)}
            finally:
                connection.close()

        checks = result['checks']
        listeners = subprocess.check_output(['/usr/sbin/lsof', '-nP', '-a', '-p', str(child.pid),
                                             '-iTCP', '-sTCP:LISTEN', '-Fn']).splitlines()
        names = [line[1:] for line in listeners if line.startswith(b'n')]
        checks['loopback_only'] = names == [f'127.0.0.1:{port}'.encode()]
        assert checks['loopback_only']
        checks['owner_health'] = request(timeout=60)
        assert checks['owner_health']['status'] == 200 and checks['owner_health']['owner_identity']
        checks['unauthenticated_health'] = request(auth=None)
        checks['browser_origin_with_capability'] = request(extra={'Origin': 'https://untrusted.invalid'})
        checks['browser_fetch_with_capability'] = request(extra={'Sec-Fetch-Site': 'cross-site'})
        checks['unauthenticated_body_not_read'] = request(auth=None, path='/inference', method='POST',
                                                        extra={'Content-Length': '1000000'})
        assert checks['unauthenticated_health']['status'] == 401
        assert checks['unauthenticated_body_not_read']['status'] == 401
        assert checks['browser_origin_with_capability']['status'] == 403
        assert checks['browser_fetch_with_capability']['status'] == 403

        baseline = counters(child)
        peak = baseline.copy()
        sent = 0
        started = time.monotonic()
        sock = socket.create_connection(('127.0.0.1', port), timeout=2)
        closed, response = False, b''
        try:
            sock.sendall(b'GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Probe: ')
            for _ in range(8 * 1024 * 1024 // 4096):
                try:
                    sock.sendall(b'a' * 4096)
                    sent += 4096
                except OSError:
                    closed = True
                    break
                if select.select([sock], [], [], .005)[0]:
                    try:
                        data = sock.recv(65536)
                    except OSError:
                        data = b''
                    response += data
                    if not data:
                        closed = True
                        break
            current = counters(child)
            peak = {k: max(peak[k], current[k]) for k in peak}
        finally:
            sock.close()
        responses.append(response)
        checks['unterminated_header'] = {'bytes_sent': sent, 'peer_closed': closed,
            'response_status': int(response.split(b' ')[1]) if response.startswith(b'HTTP/') else None,
            'seconds': round(time.monotonic() - started, 3), 'baseline': baseline, 'peak': peak,
            'owner_health_after': request()}
        assert closed and sent < 256 * 1024, 'Unfinished header was not promptly bounded'
        assert peak['rss_bytes'] - baseline['rss_bytes'] < 32 * 1024 * 1024
        assert checks['unterminated_header']['owner_health_after']['status'] == 200

        def raw_status(data):
            with socket.create_connection(('127.0.0.1', port), timeout=2) as s:
                s.sendall(data)
                content = s.recv(65536)
                responses.append(content)
                return int(content.split(b' ')[1])
        checks['oversized_complete_header'] = raw_status(b'GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nX: ' + b'a'*8192 + b'\r\n\r\n')
        checks['header_count'] = raw_status(b'GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n' + b'X: a\r\n'*64 + b'\r\n')
        checks['uri_length'] = raw_status(b'GET /' + b'a'*2048 + b' HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n')
        assert checks['oversized_complete_header'] == 400
        assert checks['header_count'] == 400
        assert checks['uri_length'] == 414

        def open_idle(count):
            sockets = []
            for _ in range(count):
                s = socket.create_connection(('127.0.0.1', port), timeout=2)
                held.append(s)
                try:
                    # Also cover peers that send no request bytes at all.
                    if len(sockets) % 2:
                        s.sendall(b'GET /health HTTP/1.1\r\n')
                except OSError:
                    pass  # An overflow socket can be rejected immediately.
                sockets.append(s)
                time.sleep(.01)
            return sockets

        def closed_by_peer(sockets):
            closed_count = 0
            for s in sockets:
                s.setblocking(False)
                try:
                    data = s.recv(65536)
                    if data:
                        responses.append(data)
                        data = s.recv(65536)
                    if not data:
                        closed_count += 1
                except BlockingIOError:
                    pass
                except OSError:
                    closed_count += 1
            return closed_count

        sockets = open_idle(16)
        time.sleep(.3)
        checks['idle_16'] = {'opened': len(sockets), 'owner_health_while_held': request(), 'counters': counters(child)}
        assert checks['idle_16']['owner_health_while_held']['status'] == 200
        # A malformed partial request gets the 10 s read deadline followed by
        # at most the 5 s keep-alive wait. Empty peers get only the latter.
        time.sleep(16)
        checks['idle_16']['peer_closed_before_client_release'] = closed_by_peer(sockets)
        checks['idle_16']['owner_health_after_deadline'] = request()
        assert checks['idle_16']['peer_closed_before_client_release'] == 16
        assert checks['idle_16']['owner_health_after_deadline']['status'] == 200
        for s in held:
            s.close()
        held.clear()

        # Deliberately exhaust the finite queue. Burst recovery must occur even
        # while clients retain their sockets; no continuous-flood guarantee.
        baseline = counters(child)
        sockets = open_idle(96)
        time.sleep(.3)
        current = counters(child)
        checks['saturation_96'] = {'opened': len(sockets), 'baseline': baseline, 'peak': current,
                                   'owner_during_burst': request()}
        thread_ceiling = checks['unterminated_header']['baseline']['threads'] + 24 + 4
        checks['saturation_96']['observed_thread_ceiling'] = thread_ceiling
        assert current['threads'] <= thread_ceiling, 'Thread growth exceeded the reviewed pool'
        # At most one additional 32-request queue behind the 32 active threads.
        time.sleep(32)
        checks['saturation_96']['peer_closed_before_client_release'] = closed_by_peer(sockets)
        checks['saturation_96']['owner_health_after_deadline'] = request()
        assert checks['saturation_96']['peer_closed_before_client_release'] == 96
        assert checks['saturation_96']['owner_health_after_deadline']['status'] == 200
        output = io.BytesIO()
        with wave.open(output, 'wb') as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(16000)
            wav.writeframes(b'\0\0' * 16000)
        audio = output.getvalue()
        boundary = 'writer-preauth-recovery'
        body = (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="silence.wav"\r\n'
                'Content-Type: audio/wav\r\n\r\n').encode() + audio
        body += f'\r\n--{boundary}--\r\n'.encode()
        checks['owner_synthetic_inference_after_burst'] = request(path='/inference', method='POST', body=body,
            extra={'Content-Type': 'multipart/form-data; boundary=' + boundary}, timeout=60)
        checks['synthetic_audio_sha256'] = hashlib.sha256(audio).hexdigest()
        assert checks['owner_synthetic_inference_after_burst']['status'] == 200
        assert checks['owner_synthetic_inference_after_burst']['owner_identity']
        checks['owner_health_at_end'] = request()
        assert checks['owner_health_at_end']['status'] == 200 and child.poll() is None
        checks['limits_marker'] = any(LIMITS_MARKER in line for line in logs)
        assert checks['limits_marker'], 'Runtime limits differ from the acceptance profile'
        backend_marker = rb'use gpu\s*= 0' if backend == 'cpu' else rb'using MTL.*backend'
        checks['backend_proved'] = re.search(backend_marker, b''.join(logs)) is not None
        assert checks['backend_proved']
        # ps output stays in memory; neither secrets nor command lines are saved.
        process = subprocess.check_output(['ps', 'eww', '-p', str(child.pid)])
        checks['credential_argv_environment_leak'] = capability.encode() in process or instance.encode() in process
        assert not checks['credential_argv_environment_leak']
        result['passed'] = True
    except Exception as error:
        result['failure_type'] = type(error).__name__
    finally:
        for s in held:
            s.close()
        child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
        for reader in readers:
            reader.join(timeout=2)
        result['owned_child_stopped'] = child.poll() is not None
        content = b''.join(logs)
        result['capability_log_or_body_leak'] = any(value.encode() in content or any(value.encode() in body for body in responses)
                                                   for value in (capability, instance))
        # The instance is intentionally returned in owner response headers;
        # only diagnostic text and bodies must exclude handshake values.
        result['worker_unchanged'] = hashlib.sha256(worker.read_bytes()).hexdigest() == before
        result['passed'] &= not result['capability_log_or_body_leak'] and result['worker_unchanged']
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--backend', choices=['cpu', 'metal'], required=True)
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('command', nargs=argparse.REMAINDER)
    a = p.parse_args()
    command = a.command[1:] if a.command[:1] == ['--'] else a.command
    if not command:
        p.error('Supply the worker command after --')
    result = run(command, a.backend)
    a.out.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'backend': a.backend, 'passed': result['passed'], 'receipt': str(a.out)}))
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
