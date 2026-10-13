#!/usr/bin/env python3
"""Run the real mobile API/HTTPS/Keystore proof on an explicit Writer emulator."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[3]
ANDROID = ROOT / 'apps/android'
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--serial', required=True)
p.add_argument('--directory', type=Path, required=True)
a = p.parse_args()
private = a.directory.resolve()
private.mkdir(parents=True, exist_ok=True, mode=0o700)
sdk = Path(os.environ.get('ANDROID_HOME', str(Path.home() / 'Android/Sdk')))
adb = [str(sdk / 'platform-tools/adb'), '-s', a.serial]
def run(args, **kwargs): return subprocess.run(args, check=True, capture_output=True, **kwargs).stdout
def command(*args, **kwargs): return run(adb + list(args), **kwargs)
if not a.serial.startswith('emulator-') or not command('emu', 'avd', 'name').startswith(b'Writer_'):
    raise SystemExit('Select an explicit Writer-owned emulator')
for port in range(18796, 18802):
    with socket.socket() as sock: sock.bind(('127.0.0.1', port))
for name in ['trusted', 'untrusted']:
    if not (private / name / 'cert.pem').exists(): run(['python3', str(ANDROID / 'scripts/mobile-https.py'), 'certificate', '--directory', str(private / name)])
wrong = private / 'wrong-host'; wrong.mkdir(exist_ok=True, mode=0o700)
if not (wrong / 'cert.pem').exists():
    run(['openssl', 'req', '-new', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(wrong / 'key.pem'), '-out', str(wrong / 'request.csr'), '-subj', '/CN=wrong emulator hostname'])
    (wrong / 'extensions').write_text('subjectAltName=IP:10.0.2.3\nbasicConstraints=CA:FALSE\n')
    run(['openssl', 'x509', '-req', '-in', str(wrong / 'request.csr'), '-CA', str(private / 'trusted/cert.pem'), '-CAkey', str(private / 'trusted/key.pem'), '-CAcreateserial', '-days', '2', '-extfile', str(wrong / 'extensions'), '-out', str(wrong / 'cert.pem')])
    (wrong / 'key.pem').chmod(0o600)
build_env = {**os.environ, 'WRITER_ANDROID_TEST_CA': str(private / 'trusted/cert.pem')}
run([str(ANDROID / 'gradlew'), '-p', str(ANDROID), '--offline', '-PwriterTestRunner=com.metrix.writer.MobileHttpsRunner', ':app:assembleDevDebug', ':app:assembleDevDebugAndroidTest'], env=build_env)
command('install', '-r', str(ANDROID / 'app/build/outputs/apk/dev/debug/app-dev-debug.apk'))
command('install', '-r', str(ANDROID / 'app/build/outputs/apk/androidTest/dev/debug/app-dev-debug-androidTest.apk'))
# No real provider credentials or inherited Writer runtime configuration in this proof.
server_env = {k: v for k, v in os.environ.items() if not k.startswith('WRITER_') and k not in ['OPENAI_API_KEY', 'GEMINI_API_KEY', 'XAI_API_KEY']}
server_env['WRITER_MOBILE_DATA_DIR'] = str(private / 'backend')
server_env['WRITER_MOBILE_PORT'] = '18796'
binary = ROOT / 'target/debug/metrix-writer-mobile'
run_id = str(uuid.uuid4())
credential = private / f'device-{run_id}.token'
run([str(binary), 'issue', str(credential)], env=server_env)
token = credential.read_text()
processes = []; logs = []
def start(args, env=None):
    log = (private / f'process-{len(processes)}.log').open('ab'); logs.append(log)
    process = subprocess.Popen(args, env=env, stdout=log, stderr=log); processes.append(process)
def ready(port):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if any(p.poll() is not None for p in processes): raise RuntimeError('Owned proof process exited; inspect private logs')
        try:
            with socket.create_connection(('127.0.0.1', port), timeout=1): return
        except OSError: time.sleep(.1)
    raise TimeoutError('Proof endpoint did not start')
try:
    start([str(binary), 'serve'], server_env); ready(18796)
    for port, name, mode in [(18797, 'trusted', 'proxy'), (18798, 'untrusted', 'proxy'), (18799, 'trusted', 'redirect'), (18800, 'trusted', 'incompatible'), (18801, 'wrong-host', 'proxy')]:
        start(['python3', str(ANDROID / 'scripts/mobile-https.py'), 'serve', '--directory', str(private / name), '--port', str(port), '--mode', mode]); ready(port)
    config = {'session_directory': 'mobile-proof-' + run_id, 'origin': 'https://10.0.2.2:18797', 'token': token, 'untrusted': 'https://10.0.2.2:18798', 'wrong_hostname': 'https://10.0.2.2:18801', 'redirect': 'https://10.0.2.2:18799', 'incompatible': 'https://10.0.2.2:18800'}
    # Pipe a disposable test credential directly into test-only private app storage; no shell argv secret.
    command('shell', 'run-as', 'com.metrix.writer.debug', 'mkdir', '-p', 'no_backup')
    command('shell', 'run-as', 'com.metrix.writer.debug', 'sh', '-c', "'cat > no_backup/mobile-proof.json'", input=json.dumps(config).encode())
    output = command('shell', 'am', 'instrument', '-w', 'com.metrix.writer.debug.test/com.metrix.writer.MobileHttpsRunner').decode()
    (private / 'connect-result.txt').write_text(output)
    if 'PASS:' not in output or 'FAIL' in output: raise RuntimeError(output)
    print(output.strip())
    run([str(binary), 'revoke', token.split('.')[1]], env=server_env)
    command('shell', 'am', 'force-stop', 'com.metrix.writer.debug')
    output = command('shell', 'am', 'instrument', '-w', '-e', 'phase', 'revoked', 'com.metrix.writer.debug.test/com.metrix.writer.MobileHttpsRunner').decode()
    (private / 'revoked-result.txt').write_text(output)
    if 'PASS:' not in output or 'FAIL' in output: raise RuntimeError(output)
    print(output.strip())
    (private / 'receipt.json').write_text(json.dumps({'serial': a.serial, 'owned_pids': [p.pid for p in processes], 'ports': list(range(18796,18802)), 'real_api': True, 'cloud_keys': False, 'transcription_proven': False, 'passed': True}, indent=2))
finally:
    command('shell', 'run-as', 'com.metrix.writer.debug', 'rm', '-f', 'no_backup/mobile-proof.json')
    for process in reversed(processes):
        if process.poll() is None: process.terminate()
        process.wait(timeout=30)
    for log in logs: log.close()
