#!/usr/bin/env python3
"""Fetch pinned Apple Silicon NeMo archives and verify the release checksums."""
import argparse
import hashlib
import json
from pathlib import Path
import tarfile
import urllib.request


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    output = args.output.resolve()
    repo = Path(__file__).resolve().parents[2]
    if output.is_relative_to(repo):
        p.error('Keep runtime archives outside source')
    output.mkdir(parents=True, exist_ok=True)
    pins = json.loads(Path(__file__).with_name('parakeet.json').read_text())['runtimes']
    for backend in ['metal', 'cpu']:
        pin = pins['macos_'+backend]
        with urllib.request.urlopen(pin['source_url']+'.sha256', timeout=60) as response:
            upstream = response.read(4096).decode().split()[0]
        if upstream != pin['sha256']:
            raise ValueError('Upstream release checksum differs from reviewed pin')
        archive = output/pin['file_name']
        if not archive.exists():
            partial = archive.with_suffix('.part')
            urllib.request.urlretrieve(pin['source_url'], partial)
            partial.rename(archive)
        if archive.stat().st_size != pin['bytes'] or hashlib.file_digest(archive.open('rb'), 'sha256').hexdigest() != pin['sha256']:
            raise ValueError('Pinned NeMo archive mismatch')
        destination = output/('nemo-'+backend)
        destination.mkdir(exist_ok=False)
        with tarfile.open(archive) as tar:
            tar.extractall(destination, filter='data')
        print(f'{backend}: archive and upstream .sha256 verified')


if __name__ == '__main__':
    main()
