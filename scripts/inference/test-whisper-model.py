#!/usr/bin/env python3
"""Regression inputs for Writer's model parser; uses only fabricated invalid models.
Pass a whisper-cli or whisper-server binary. A valid model/inference fixture is
validated separately; this command never opens a listening server.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import struct
import subprocess


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    magic = struct.pack('<I', 0x67676d6c)
    hparams = [51865, 1500, 384, 6, 4, 448, 384, 6, 4, 80, 1]
    header = magic + struct.pack('<11i', *hparams)
    cases = {'empty': b'', 'bad-magic': b'bad model', 'magic-only': magic,
             'partial-hparams': header[:31], 'missing-filters': header,
             'overflow-filters': header + struct.pack('<ii', 0x7fffffff, 0x7fffffff),
             'truncated-filters': header + struct.pack('<ii', 80, 201)}
    bad = hparams.copy(); bad[3] = 0
    cases['zero-heads'] = magic + struct.pack('<11i', *bad)
    filters = header + struct.pack('<ii', 80, 201) + bytes(80 * 201 * 4)
    cases['negative-vocabulary'] = filters + struct.pack('<i', -1)
    cases['oversized-token'] = filters + struct.pack('<iI', 1, 1024 * 1024 + 1)
    cases['truncated-token'] = filters + struct.pack('<iI', 1, 4) + b'ab'
    tensors = filters + struct.pack('<i', 0)
    cases['missing-tensors'] = tensors
    cases['partial-tensor-header'] = tensors + b'\x01'
    cases['invalid-tensor-type'] = tensors + struct.pack('<iii', 2, 1, -1)
    cases['invalid-tensor-dimensions'] = tensors + struct.pack('<iiiii', 2, 1, 0, -1, 384)
    name = b'encoder.positional_embedding'
    cases['truncated-tensor-data'] = (tensors + struct.pack('<iiiii', 2, len(name), 0, 384, 1500)
                                      + name + bytes(4))
    env = os.environ.copy()
    env.update(ASAN_OPTIONS='detect_leaks=1:halt_on_error=1', UBSAN_OPTIONS='halt_on_error=1')
    results = []
    for name, data in cases.items():
        model = args.output/(name+'.bin'); model.write_bytes(data)
        model_path = str(model.resolve())
        if os.name != 'nt' and args.binary.suffix == '.exe':
            model_path = subprocess.check_output(['wslpath', '-w', model_path], text=True).strip()
        command = [str(args.binary), '-m', model_path]
        if args.binary.stem == 'whisper-cli':
            command += ['-f', model_path]  # CLI requires a file; invalid models fail before audio is read.
        # A private worker reads its launch handshake before loading anything (writer-auth.h).
        handshake = (secrets.token_hex(32)+'\n'+secrets.token_hex(32)+'\n').encode()
        result = subprocess.run(command, capture_output=True, timeout=30, env=env,
                                input=handshake if args.binary.stem == 'whisper-server' else b'')
        error = result.stderr.decode(errors='replace')
        assert result.returncode in (1, 3), (name, result.returncode, error)
        assert not any(s in error for s in ['AddressSanitizer', 'runtime error:', 'LeakSanitizer']), error
        assert 'failed to' in error and 'model' in error, (name, error)
        assert model.read_bytes() == data
        results.append({'case': name, 'exit': result.returncode, 'input_sha256': hashlib.sha256(data).hexdigest()})
    (args.output/'receipt.json').write_text(json.dumps({'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                                                      'cases': results}, indent=2)+'\n')
    print(f'{len(results)} invalid models rejected; no sanitizer reports; inputs unchanged')


if __name__ == '__main__':
    main()
