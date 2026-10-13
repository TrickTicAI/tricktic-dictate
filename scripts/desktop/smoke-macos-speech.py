#!/usr/bin/env python3
"""Native release gate: exact bundled workers must authenticate and infer synthetic audio."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--resources', type=Path, required=True)
p.add_argument('--whisper-model', type=Path, required=True)
p.add_argument('--nemo-model', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
spec = importlib.util.spec_from_file_location('worker_smoke', Path(__file__).parents[1] / 'inference/worker-auth-smoke.py')
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
results = []
preauth_results = []
spec = importlib.util.spec_from_file_location('preauth_smoke', Path(__file__).parents[1] / 'inference/worker-preauth-macos.py')
preauth = importlib.util.module_from_spec(spec)
spec.loader.exec_module(preauth)
for family in ['whisper', 'nemo']:
    for backend in ['metal', 'cpu']:
        if family == 'whisper':
            relative = 'runtime/whisper/' + ('cpu/' if backend == 'cpu' else '') + 'whisper-server'
            command = [str(a.resources / relative), '-m', str(a.whisper_model.resolve()), '-l', 'en', '-t', '4', '-nt', '-ac', '768', '--port', '0']
            if backend == 'cpu':
                command.append('-ng')
        else:
            relative = f'runtime/nemo/{backend}/bin/nemo-speech'
            command = [str(a.resources / relative), 'serve', '--asr-model', str(a.nemo_model.resolve()), '--backend', backend, '--no-ui', '--port', '0', '--threads', '2']
        print(f'Checking {family} {backend}: private handshake, denied foreign requests, synthetic inference and restart', flush=True)
        old = smoke.run(command, family, backend=backend)
        smoke.run(command, family, old, backend=backend)
        results.append({'path': relative, 'sha256': hashlib.sha256((a.resources / relative).read_bytes()).hexdigest(), 'backend': backend, 'launches': 2, 'passed': True})
        if family == 'whisper':
            print(f'Checking Whisper {backend}: bounded unauthenticated headers, idle sockets and queue recovery', flush=True)
            evidence = preauth.run(command, backend)
            preauth_results.append(evidence)
            if not evidence['passed']:
                a.output.write_text(json.dumps({'version': 2, 'workers': results, 'preauthentication': preauth_results}, indent=2) + '\n')
                p.exit(1, 'Packaged Whisper pre-authentication resource acceptance failed; receipt retained.\n')
a.output.write_text(json.dumps({'version': 2, 'workers': results, 'preauthentication': preauth_results}, indent=2) + '\n')
print('All four packaged speech workers passed.', flush=True)
