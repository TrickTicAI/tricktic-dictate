#!/usr/bin/env python3
"""Prepare an unapproved release manifest; native source attribution is manual."""
import argparse
import json
from pathlib import Path

from artifacts import PLATFORMS
from inventory import digest
from scanning import ScanError, command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=PLATFORMS, required=True)
    parser.add_argument('--payload', type=Path, required=True, help='fully extracted final artifact payload')
    parser.add_argument('--artifact', type=Path, action='append', required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    base = args.out.resolve().parent
    payload = args.payload.resolve()
    try:
        if args.out.exists() or not payload.is_dir() or not payload.is_relative_to(base):
            raise ScanError('output must be new and payload must be contained beside it')
        files = {}
        for path in sorted(payload.rglob('*')):
            if path.is_symlink():
                raise ScanError('stage a dereferenced payload before preparing the manifest')
            if path.is_file():
                files[path.relative_to(payload).as_posix()] = {'bytes': path.stat().st_size, 'sha256': digest(path)}
        artifacts = []
        for path in args.artifact:
            path = path.resolve()
            if not path.is_relative_to(base) or not path.is_file():
                raise ScanError('artifacts must be files contained beside the manifest')
            artifacts.append({'path': path.relative_to(base).as_posix(), 'sha256': digest(path)})
        root = Path(__file__).resolve().parents[2]
        data = {'schema_version': 1, 'platform': args.platform,
                'source_commit': command(['git', 'rev-parse', 'HEAD'], root).stdout.decode().strip(),
                'builder': '', 'extraction_method': '', 'payload': payload.relative_to(base).as_posix(),
                'artifacts': artifacts, 'files': files, 'components': []}
        args.out.write_text(json.dumps(data, indent=2, sort_keys=True) + '\n')
        print('Draft manifest prepared. Supply builder/extraction evidence and native component attribution before release scanning.')
    except Exception as error:
        parser.exit(2, (str(error) if isinstance(error, ScanError) else 'invalid manifest input') + '\n')


if __name__ == '__main__':
    main()
