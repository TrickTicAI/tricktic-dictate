#!/usr/bin/env python3
"""Native private text-server auth/ownership checks with synthetic requests.

Uses only the task's explicitly verified public model fixture. Never records
worker output, credentials or generated text. This is separate from production
adapter and final signed-bundle acceptance.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import secrets
import socket
import subprocess
import time
import urllib.error
import urllib.request

MODEL = 'qwen3-4b-instruct-q4.gguf'
MODEL_SHA = '8cdb57cbb880d313736a9bc4e3d3d2485f145b5e19cf33783746e753e82641fc'


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--runtime', type=Path, required=True)
    p.add_argument('--models', type=Path, required=True)
    p.add_argument('--backend', choices=['cpu', 'metal'], required=True)
    p.add_argument('--out', type=Path, required=True)
    a = p.parse_args()
    root = Path(__file__).resolve().parents[3]
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        p.error('Requires actual Apple Silicon native execution')
    if a.out.exists() or not a.out.resolve().is_relative_to(root / '.local'):
        p.error('Use a new ignored output path in this task worktree')
    marker = json.loads((a.models / 'download-receipt.json').read_text())
    if marker.get('pinned_public_fixture_verified') is not True or marker.get('sha256') != MODEL_SHA:
        p.error('Use the separately downloaded private public-model fixture')
    if set(f.name for f in a.models.iterdir()) != {MODEL, 'download-receipt.json'}:
        p.error('The private model fixture directory contains unexpected data')
    model = a.models / MODEL
    if model.is_symlink() or digest(model) != MODEL_SHA:
        p.error('Private model bytes differ from the pin')
    binary = (a.runtime / 'llama-server').resolve(strict=True)
    result = {'kind': 'native-private-text-boundaries', 'backend': a.backend,
              'binary_sha256': digest(binary), 'model_sha256': MODEL_SHA, 'checks': {}}
    token, alias = secrets.token_hex(32), 'writer-private-' + secrets.token_hex(8)
    key = a.models / ('.writer-boundary-key-' + secrets.token_hex(12))
    with open(key, 'x', opener=lambda path, flags: os.open(path, flags, 0o600)) as f:
        f.write(token)
    child = None
    sockets = []
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        result['step'] = 'start-private-worker'
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        command = [str(binary), '--host', '127.0.0.1', '--port', str(port), '-m', MODEL,
                   '-c', '8192', '-t', '4', '-ngl', '0' if a.backend == 'cpu' else '999',
                   '-lv', '4', '--parallel', '1', '--no-webui', '--alias', alias,
                   '--api-key-file', key.name]
        child = subprocess.Popen(command, cwd=a.models, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        address = 'http://127.0.0.1:' + str(port)

        def request(path, credential=None, body=None):
            headers = {'Authorization': 'Bearer ' + credential} if credential is not None else {}
            if body is not None:
                headers['Content-Type'] = 'application/json'
            req = urllib.request.Request(address + path, data=body, headers=headers)
            try:
                with opener.open(req, timeout=5) as response:
                    return response.status, response.read(1024 * 1024)
            except urllib.error.HTTPError as e:
                return e.code, e.read(4096)

        until = time.monotonic() + 180
        result['step'] = 'authenticated-readiness'
        while True:
            if child.poll() is not None:
                raise RuntimeError('Private worker exited before readiness')
            try:
                status, body = request('/v1/models', token)
                if status == 200 and any(x.get('id') == alias for x in json.loads(body)['data']):
                    break
            except (OSError, ValueError, KeyError):
                pass
            if time.monotonic() >= until:
                raise RuntimeError('Private worker readiness timed out')
            time.sleep(.2)
        listeners = subprocess.check_output(['/usr/sbin/lsof', '-nP', '-a', '-p', str(child.pid),
                                             '-iTCP', '-sTCP:LISTEN', '-Fn'], text=True)
        names = [s[1:] for s in listeners.splitlines() if s.startswith('n')]
        if names != ['127.0.0.1:' + str(port)]:
            raise RuntimeError('Private worker listener ownership differs')
        result['checks']['owned_loopback_listener'] = True
        result['step'] = 'credential-refusal'
        for name, credential in [('missing', None), ('wrong', secrets.token_hex(32))]:
            status, _ = request('/v1/models', credential)
            result['checks'][name + '_credential_status'] = status
            if status != 401:
                raise RuntimeError('Private worker failed credential refusal')
        # With a limited number of incomplete private clients, another valid
        # request must still complete. This is a bounded responsiveness check,
        # not a claim about arbitrary concurrent-load capacity.
        for _ in range(2):
            s = socket.create_connection(('127.0.0.1', port), timeout=5)
            s.sendall(b'GET /v1/models HTTP/1.1\r\nHost: localhost\r\n')
            sockets.append(s)
        result['step'] = 'bounded-incomplete-client-responsiveness'
        status, _ = request('/v1/models', token)
        if status != 200:
            raise RuntimeError('Valid request failed beside two incomplete clients')
        result['checks']['valid_request_beside_two_incomplete_clients'] = True
        result['step'] = 'disabled-reranking'
        status, body = request('/rerank', token, b'{"top_n":-1,"query":"synthetic","documents":["synthetic"]}')
        result['checks']['disabled_rerank_status'] = status
        error = json.loads(body)
        error = error.get('error', error)
        # The pinned upstream disabled-feature guard deliberately returns 501.
        # Admit that exact response, never an arbitrary server error or crash.
        if (status != 501 or error.get('code') != 501
                or error.get('type') != 'not_supported_error'
                or error.get('message') != 'This server does not support reranking. Start it with `--reranking`'):
            raise RuntimeError('Unexpected disabled reranking response')
        result['checks']['exact_disabled_reranking_guard'] = True
        status, _ = request('/v1/models', token)
        if status != 200:
            raise RuntimeError('Private worker did not remain healthy after rejected reranking')
        result['checks']['healthy_after_rejected_reranking'] = True
        result['step'] = 'completed'
    except Exception as e:
        result['failure_kind'] = type(e).__name__
    finally:
        for s in sockets:
            s.close()
        if child is not None:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=10)
            result['worker_stopped'] = child.poll() is not None
        key.unlink()
        result['temporary_key_removed'] = not key.exists()
        result['model_hash_unchanged'] = digest(model) == MODEL_SHA
        result['passed'] = ('failure_kind' not in result and result.get('worker_stopped') is True
                            and result['temporary_key_removed'] and result['model_hash_unchanged'])
        a.out.parent.mkdir(parents=True, exist_ok=True)
        with a.out.open('x') as f:
            f.write(json.dumps(result, indent=2) + '\n')
    print('Native private text boundary fixture:', 'passed' if result['passed'] else 'failed')
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
