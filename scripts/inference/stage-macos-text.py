#!/usr/bin/env python3
"""Stage the pinned fixed Mac text source without modifying the input clone.

The new local clone has its own Git metadata and a detached upstream identity.
Only the pinned miniaudio header changes; no source or binary is downloaded here.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def stage(source, miniaudio, miniaudio_license, output, pin):
    source = source.resolve(strict=True)
    output = output.resolve()
    if output.exists() or output.is_relative_to(source) or output.is_relative_to(ROOT):
        raise ValueError('Use a new staging path outside source and the input clone')
    header = miniaudio.read_bytes()
    license_text = miniaudio_license.read_bytes()
    if hashlib.sha256(header).hexdigest() != pin['miniaudio']['sha256']:
        raise ValueError('miniaudio.h differs from the reviewed pin')
    if hashlib.sha256(license_text).hexdigest() != pin['miniaudio']['license_sha256']:
        raise ValueError('miniaudio license differs from the reviewed pin')
    subprocess.run(['git', '-C', str(source), 'cat-file', '-e', pin['llama']['revision'] + '^{commit}'], check=True)
    output.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['git', 'clone', '--no-hardlinks', '--no-checkout', str(source), str(output)], check=True)
    subprocess.run(['git', '-c', 'core.hooksPath=/dev/null', '-C', str(output),
                    'checkout', '--detach', pin['llama']['revision']], check=True)
    vendored = output / 'vendor/miniaudio/miniaudio.h'
    original = hashlib.sha256(vendored.read_bytes()).hexdigest()
    vendored.write_bytes(header)
    (output / 'writer-miniaudio-LICENSE.txt').write_bytes(license_text)
    receipt = {'llama': pin['llama'], 'miniaudio': pin['miniaudio'],
               'vendored_miniaudio_original_sha256': original,
               'staged_miniaudio_sha256': hashlib.sha256(header).hexdigest(),
               'staged_miniaudio_license_sha256': hashlib.sha256(license_text).hexdigest(),
               'input_clone_modified': False}
    (output / 'writer-source.json').write_text(json.dumps(receipt, indent=2) + '\n')
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--miniaudio', required=True, type=Path)
    parser.add_argument('--miniaudio-license', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    pin = json.loads((ROOT / 'scripts/inference/macos-sources.json').read_text())
    stage(args.source, args.miniaudio, args.miniaudio_license, args.output, pin)
    print(args.output.resolve())


if __name__ == '__main__':
    main()
