#!/usr/bin/env python3
"""Bubble Stop → protected HTTPS API → exact editor readback, on an owned emulator.

Default is explicitly fake cloud responses. --live requires a private Gemini key
file and an explicit consented/public PCM16 WAV suite. No provider key reaches Android.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
import time
import uuid
import wave

import dev


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--serial', required=True)
    p.add_argument('--directory', type=Path, required=True)
    p.add_argument('--live', action='store_true')
    p.add_argument('--companion', action='store_true', help='Exercise MW-61 companion screens instead of bubble delivery')
    p.add_argument('--processed-delivery', action='store_true', help='Fixture bubble suite inserts dictionary/cleaned output')
    p.add_argument('--snippet-trigger', default='Fixture', help='Exact spoken trigger in the first live clip')
    p.add_argument('--snippet-trigger-file', type=Path, help='Private UTF-8 trigger file for live speech')
    p.add_argument('--key-file', type=Path)
    p.add_argument('--suite', type=Path)
    a = p.parse_args()
    dev.selected(a.serial); owner = dev.owned(a.serial)
    if not a.serial.startswith('emulator-'): raise ValueError('Owned emulator required')
    if a.processed_delivery and (a.live or a.companion): raise ValueError('--processed-delivery is a fixture bubble mode')
    if a.live and not (a.key_file and a.suite): raise ValueError('Live requires --key-file and --suite')
    private = a.directory.resolve(); private.mkdir(parents=True, exist_ok=True, mode=0o700)
    root = dev.ROOT.parents[1]
    def run(args, **kw): return subprocess.run(args, check=True, capture_output=True, timeout=300, **kw).stdout
    def adb(*args, **kw): return run([dev.tool('platform-tools/adb'), '-s', a.serial, *args], **kw)
    for port in (18795, 18796, 18797):
        with socket.socket() as sock:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(('127.0.0.1', port))
    cert = private / 'tls'
    if not (cert / 'cert.pem').exists():
        run(['python3', str(dev.ROOT / 'scripts/mobile-https.py'), 'certificate', '--directory', str(cert)])
    build_env = {**os.environ, 'WRITER_ANDROID_TEST_CA': str(cert / 'cert.pem')}
    runner = 'com.metrix.writer.CompanionRunner' if a.companion else 'com.metrix.writer.CloudDictationRunner'
    output = run([str(dev.ROOT / 'gradlew'), '-p', str(dev.ROOT), '--offline',
        '-PwriterTestRunner=' + runner, ':app:assembleDevDebug', ':app:assembleDevDebugAndroidTest', ':editor:assembleDebug'], env=build_env)
    (private / 'build.log').write_bytes(output)
    for apk in ('app/build/outputs/apk/dev/debug/app-dev-debug.apk', 'app/build/outputs/apk/androidTest/dev/debug/app-dev-debug-androidTest.apk', 'editor/build/outputs/apk/debug/editor-debug.apk'):
        adb('install', '-r', str(dev.ROOT / apk))
    processes = []; logs = []; fixture = None
    def start(args, env=None):
        log = (private / f'process-{len(processes)}.log').open('ab'); logs.append(log)
        process = subprocess.Popen(args, env=env, stdout=log, stderr=log); processes.append(process)
        return process
    def ready(port):
        for _ in range(300):
            if any(proc.poll() is not None for proc in processes): raise RuntimeError('Owned process exited; inspect private logs')
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=1): return
            except OSError: time.sleep(.1)
        raise TimeoutError('Endpoint startup failed')
    token = 'mw1.' + str(uuid.uuid4()) + '.' + uuid.uuid4().hex + uuid.uuid4().hex
    models = [{'selection': {'provider': 'gemini', 'model': 'fixture-cloud'}, 'input_languages': ['en', 'he', 'mixed'], 'configured': True, 'available': True, 'execution_location': 'cloud', 'status': 'available'}]
    state = {'mode': 'success'}; recordings = {}; jobs = {}; events = []
    class Fixture(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def body(self):
            if self.headers.get('Transfer-Encoding') == 'chunked':
                blocks = []
                while True:
                    size = int(self.rfile.readline().split(b';')[0], 16)
                    if not size: self.rfile.readline(); return b''.join(blocks)
                    blocks.append(self.rfile.read(size)); self.rfile.read(2)
            return self.rfile.read(int(self.headers.get('Content-Length', 0)))
        def send(self, data, code=200):
            data = json.dumps({'api_version': '1', 'data': data}).encode()
            self.send_response(code); self.send_header('Content-Type', 'application/json'); self.send_header('Content-Length', str(len(data))); self.end_headers(); self.wfile.write(data)
        def do_POST(self):
            body = self.body()
            if self.path == '/control': state.update(json.loads(body)); self.send({}); return
            if self.headers.get('Authorization') != 'Bearer ' + token: self.send({}, 401); return
            mode = state['mode']; events.append({'path': self.path, 'mode': mode})
            if self.path == '/api/v1/recordings':
                boundary = self.headers['Content-Type'].split('boundary=')[1].encode()
                parts = body.split(b'--' + boundary)
                metadata = json.loads(parts[1].split(b'\r\n\r\n', 1)[1].strip())['data']
                audio = parts[2].split(b'\r\n\r\n', 1)[1][:-2]
                recordings[metadata['id']] = {'metadata': metadata, 'audio': {'state': 'uploading' if mode == 'partial' else 'saved'}}
                if mode == 'partial': self.close_connection = True; return
                assert len(audio) == metadata['byte_length']
                self.send(recordings[metadata['id']]); return
            if self.path.endswith('/cleanup'):
                if mode == 'cleanup-drop': self.close_connection = True; return
                job = next(j for j in jobs.values() if j['id'] == self.path.split('/')[-2])
                job['status']['result']['cleanup'] = {'state': 'failed', 'error': {'message': 'Deliberate cleanup failure'}} if mode == 'cleanup-failed' else {'state': 'succeeded', 'cleaned_text': 'חתימה 🌍\nAlex exact שלום Metrix 🌍.', 'selection': json.loads(body)['data']}
                self.send(job); return
            if self.path == '/api/v1/transcriptions':
                request = json.loads(body)['data']; key = request['idempotency_key']
                if key not in jobs:
                    status = {'state': 'failed', 'error': {'message': 'Deliberate fixture failure'}} if mode == 'failed' else {'state': 'succeeded', 'result': {'transcript': {'raw_text': 'Fixture exact שלום English 🌍'}}}
                    jobs[key] = {'id': str(uuid.uuid4()), 'request': request, 'status': status}
                job = jobs[key]
                if mode == 'drop': self.close_connection = True; return
                if mode == 'server': self.send({}, 503); return
                if mode == 'wrong':
                    job = json.loads(json.dumps(job)); job['request']['idempotency_key'] = str(uuid.uuid4())
                if mode == 'delay': time.sleep(4)
                self.send(job); return
            self.send({}, 404)
        def do_GET(self):
            if self.headers.get('Authorization') != 'Bearer ' + token: self.send({}, 401); return
            if self.path == '/api/v1/mobile/connection': self.send({'service': 'metrix-writer-mobile', 'mobile_version': 1, 'models': models}); return
            if self.path == '/api/v1/cleanup/providers': self.send([{'selection': {'provider': 'gemini', 'model': 'gemini-3.8-flash'}, 'configured': True, 'execution_location': 'cloud'}]); return
            if self.path.endswith('/processed'): self.send({'text': 'חתימה 🌍\nAlex exact שלום Metrix 🌍'}); return
            if self.path.startswith('/api/v1/jobs/'): self.send(next(j for j in jobs.values() if j['id'] == self.path.split('/')[-1])); return
            item = recordings.get(self.path.rsplit('/', 1)[-1]); self.send(item or {}, 200 if item else 404)
    try:
        if a.live:
            env = {k: v for k, v in os.environ.items() if not k.startswith('WRITER_') and k not in ('GEMINI_API_KEY', 'OPENAI_API_KEY', 'XAI_API_KEY')}
            env.update(WRITER_MOBILE_DATA_DIR=str(private / 'backend'), WRITER_MOBILE_PORT='18796', GEMINI_API_KEY=a.key_file.read_text().strip())
            binary = root / 'target/debug/metrix-writer-mobile'
            credential = private / ('device-' + str(uuid.uuid4()) + '.token')
            run([str(binary), 'issue', str(credential)], env=env); token = credential.read_text().strip()
            start([str(binary), 'serve'], env); ready(18796)
            clips = json.loads(a.suite.read_text())['clips']; model = 'gemini/gemini-3.8-flash'
        else:
            fixture = http.server.ThreadingHTTPServer(('127.0.0.1', 18795), Fixture)
            threading.Thread(target=fixture.serve_forever, daemon=True).start()
            wav = private / 'tone.wav'
            with wave.open(str(wav), 'wb') as out:
                out.setparams((1, 2, 16000, 0, 'NONE', 'not compressed')); out.writeframes(b'\x10\x00' * 16000)
            clips = [{'id': 'fixture-' + mode, 'mode': mode, 'wav': str(wav), 'consented': True, 'provenance': 'synthetic tone; fixture response only'} for mode in ('en', 'he', 'mixed')]
            model = 'gemini/fixture-cloud'
        start(['python3', str(dev.ROOT / 'scripts/mobile-https.py'), 'serve', '--directory', str(cert), '--port', '18797', '--upstream-port', '18796' if a.live else '18795']); ready(18797)
        adb('shell', 'run-as', dev.PACKAGE, 'mkdir', '-p', 'no_backup/cloud-proof')
        def put(name, data): adb('shell', 'run-as', dev.PACKAGE, 'sh', '-c', "'cat > no_backup/cloud-proof/" + name + "'", input=data)
        inputs = []
        for i, clip in enumerate(clips):
            if not clip.get('consented'): raise ValueError('Every clip must be explicitly public/consented')
            data = Path(clip['wav']).read_bytes()
            with wave.open(str(clip['wav']), 'rb') as wav:
                if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) != (1, 2, 16000) or data[36:40] != b'data': raise ValueError('Canonical 44-byte-header PCM16 mono 16kHz WAV required')
            name = f'clip-{i}.wav'; put(name, data)
            inputs.append({**clip, 'file': name, 'sha256': hashlib.sha256(data).hexdigest()})
        config = {'origin': 'https://10.0.2.2:18797', 'token': token, 'selection': model, 'clips': inputs, 'live': a.live, 'processed_delivery': a.processed_delivery, 'snippet_trigger': a.snippet_trigger_file.read_text().strip() if a.snippet_trigger_file else a.snippet_trigger, 'control': 'http://10.0.2.2:18795/control'}
        put('config.json', json.dumps(config).encode())
        for phase in (['run'] if a.live and not a.companion else ['run', 'resume']):
            if phase == 'resume': adb('shell', 'am', 'force-stop', dev.PACKAGE)
            output = adb('shell', 'am', 'instrument', '-w', '-e', 'phase', phase, dev.PACKAGE + '.test/' + runner)
            (private / f'{phase}.txt').write_bytes(output)
            if b'PASS:' not in output or b'FAIL' in output:
                (private / 'failure.txt').write_bytes(adb('shell', 'run-as', dev.PACKAGE, 'cat', 'no_backup/cloud-proof/failure.txt'))
                raise RuntimeError(f'{phase} failed; inspect {private}')
            print(output.decode().strip())
        receipt = adb('shell', 'run-as', dev.PACKAGE, 'cat', 'no_backup/cloud-proof/receipt.json')
        (private / 'receipt.private.json').write_bytes(receipt)
        (private / 'inputs.private.json').write_text(json.dumps(inputs, indent=2))
        # Content-free publicable receipt. Exact text stays in private app/host storage.
        records = json.loads(receipt)
        sanitized = [{'mode': r['mode'], 'clip': r['clip'], 'session_id': r['session_id'], 'job_id': r['job_id'], 'timing': r['delivery'].get('timing', r['timing']), 'exact_delivery': r['exact_delivery']} for r in records]
        (private / 'result.json').write_text(json.dumps({'live_provider': a.live, 'model': model, 'serial': a.serial, 'emulator_pid': owner['pid'], 'passed': True, 'samples': sanitized}, indent=2))
        print(json.dumps({'passed': True, 'live_provider': a.live, 'private_receipt': str(private)}))
    finally:
        try:
            adb('shell', 'run-as', dev.PACKAGE, 'rm', '-f', 'no_backup/cloud-proof/config.json')
        finally:
            if fixture: fixture.shutdown(); fixture.server_close()
            for proc in reversed(processes):
                if proc.poll() is None: proc.terminate()
                proc.wait(timeout=30)
            for log in logs: log.close()
            (private / 'fixture-events.json').write_text(json.dumps(events, indent=2))


if __name__ == '__main__': main()
