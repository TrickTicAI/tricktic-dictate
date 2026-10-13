#!/usr/bin/env python3
"""Complete the security release manifest for one extracted Windows installer (MW-SEC-21).

Attribution comes from windows-components.json. Every native file in the payload
must be covered and every component pattern must match; otherwise nothing is
written. The result is operator-attested input for scripts/security/gate.py.
"""
import argparse
import fnmatch
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts/security'))
from artifacts import native_file  # noqa: E402


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


# installer.nsh extracts Writer's own items into this folder and renames them
# into place (MW-SEC-26), so 7-Zip shows them under it. Patterns name the
# installed location; manifest paths stay as extracted for the gate.
STAGE = '.writer-staged/'


def matches(path, pattern):
    parts, wanted = path.removeprefix(STAGE).split('/'), pattern.split('/')
    return len(parts) == len(wanted) and all(fnmatch.fnmatchcase(p, w) for p, w in zip(parts, wanted))


def components(spec, paths, commit):
    out, covered = [], set()
    for item in spec['components']:
        item = json.loads(json.dumps(item).replace('@commit', commit))
        files = sorted({path for path in paths for pattern in item['files'] if matches(path, pattern)})
        unmatched = [p for p in item['files'] if not any(matches(path, p) for path in paths)]
        if unmatched:
            raise SystemExit(f"{item['name']}: pattern matches nothing: {unmatched}")
        item['files'] = files
        if 'source' in item:
            item['osv_query'] = {'commit': item['source']['revision']}
        covered.update(files)
        out.append(item)
    return out, covered


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--payload', type=Path, required=True, help='7-Zip extraction of the installer, beside --out')
    parser.add_argument('--installer', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    base = args.out.resolve().parent
    payload, installer = args.payload.resolve(), args.installer.resolve()
    if args.out.exists() or not payload.is_relative_to(base) or not installer.is_relative_to(base):
        raise SystemExit('output must be new; payload and installer must be beside it')
    receipt = json.loads(Path(f'{installer}.receipt.json').read_text())
    if receipt.get('artifact', {}).get('sha256') != digest(installer):
        raise SystemExit('installer does not match its receipt')
    files = {}
    for path in sorted(payload.rglob('*')):
        if path.is_symlink():
            raise SystemExit('payload must not contain symlinks')
        if path.is_file():
            files[path.relative_to(payload).as_posix()] = {'bytes': path.stat().st_size, 'sha256': digest(path)}
    spec = json.loads((Path(__file__).parent / 'windows-components.json').read_text())
    items, covered = components(spec, list(files), receipt['commit'])
    missing = [name for name in files if native_file(payload / name) and name not in covered]
    if missing:
        raise SystemExit(f'native files without attribution: {missing}')
    head = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip()
    data = {'schema_version': 1, 'platform': 'windows', 'source_commit': head,
            'builder': (f"pnpm desktop:installer --release --without-authenticode on the prepared WSL host; "
                        f"build commit {receipt['commit']}; receipt releaseType {receipt['releaseType']}; "
                        f"runtime lock {receipt['runtimeLockSha256']}; tools {json.dumps(receipt.get('tools', {}), sort_keys=True)}"),
            'extraction_method': '7-Zip 7z.exe x on Windows: NSIS payload unpacked without running the installer',
            'payload': payload.relative_to(base).as_posix(),
            'artifacts': [{'path': installer.relative_to(base).as_posix(), 'sha256': digest(installer)}],
            'files': files, 'components': items}
    args.out.write_text(json.dumps(data, indent=2, sort_keys=True) + '\n')
    print(f'Manifest written: {len(files)} files, {len(items)} components, source {head}, build {receipt["commit"]}.')


if __name__ == '__main__':
    main()
