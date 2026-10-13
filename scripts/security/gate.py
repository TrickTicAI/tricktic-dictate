#!/usr/bin/env python3
"""Local security commands; no release publication or CI integration."""
import argparse
import datetime as dt
import importlib.metadata
import json
import os
from pathlib import Path
import sys

from artifacts import release_input
from inventory import digest, inventory, sbom
import personal_paths
from scanning import ScanError, command, now, osv, policy_findings, secrets


ROOT = Path(__file__).resolve().parents[2]


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.tmp')
    temporary.write_text(json.dumps(data, indent=2, sort_keys=True) + '\n')
    os.chmod(temporary, 0o600)
    temporary.replace(path)


def source_receipt(root):
    commit = command(['git', 'rev-parse', 'HEAD'], root).stdout.decode().strip()
    tracked = command(['git', 'ls-files', '-z'], root).stdout.decode().split('\0')
    paths = [p for p in tracked if p and (p.endswith(('Cargo.lock', 'pnpm-lock.yaml', 'gradle.lockfile',
              'runtime.lock.json', 'Cargo.toml', 'package.json', 'requirements.txt')) or p.startswith('scripts/security/'))]
    return {'commit': commit,
            'dirty': bool(command(['git', 'status', '--porcelain', '--untracked-files=normal'], root).stdout),
            'input_sha256': {p: digest(root / p) for p in sorted(paths)}}


def scan(args):
    policy = json.loads((ROOT / 'scripts/security/policy.json').read_text())
    source = source_receipt(ROOT)
    report = {'schema_version': 1, 'kind': args.command, 'created_at': now(), 'source': source,
              'platform': args.platform, 'status': 'incomplete', 'findings': [],
              'tools': {'python': sys.version.split()[0], 'PyYAML': importlib.metadata.version('PyYAML')}}
    manifest_path = Path(args.manifest).resolve() if args.manifest else None
    manifest, payload, artifacts, native = None, None, [], []
    try:
        if args.command == 'release':
            if source['dirty']:
                raise ScanError('release gate requires a clean committed source tree')
            if not manifest_path:
                raise ScanError('release gate requires --manifest')
            manifest, payload, artifacts, native = release_input(manifest_path, source['commit'])
            args.platform = manifest['platform']
            report['platform'] = args.platform
            report['manifest_sha256'] = digest(manifest_path)
            report['artifact_evidence'] = manifest
        entries = inventory(ROOT, args.ecosystem, args.platform)
        if manifest:
            entries = [e for e in entries if e['ecosystem'] != 'native'] + native
        report['inventory'] = entries
        report['sbom'] = sbom(entries, args.platform, source['commit'])
        if manifest:
            for name, info in manifest['files'].items():
                report['sbom']['components'].append({'type': 'file', 'bom-ref': 'payload:' + name,
                    'name': name, 'hashes': [{'alg': 'SHA-256', 'content': info['sha256']}]})
            for info in manifest['artifacts']:
                report['sbom']['components'].append({'type': 'file', 'bom-ref': 'artifact:' + info['path'],
                    'name': info['path'], 'hashes': [{'alg': 'SHA-256', 'content': info['sha256']}]})
        if args.command in ('scan', 'release'):
            findings, database = osv(entries)
            report['findings'], report['advisory_database'] = findings, database
            # A file hash is not an upstream dependency identity and cannot be
            # quietly treated as a clean advisory scan.
            for entry in entries:
                if entry['ecosystem'] == 'native' and not entry.get('osv_query'):
                    report['findings'].append({'id': 'NATIVE-PROVENANCE-UNKNOWN', 'purl': entry['purl'],
                                              'severity': 'unknown', 'scopes': entry['scopes'],
                                              'modified': 'inventory-v1', 'reachability': 'unreviewed',
                                              'evidence': entry['evidence']})
            report['blocked'] = policy_findings(report['findings'], policy, args.platform)
        else:
            # Expired/malformed policy must also fail inventory and secrets commands.
            policy_findings([], policy, args.platform)
            report['blocked'] = []
        if args.command in ('secrets', 'release'):
            report['secrets'] = secrets(ROOT, policy, payload, artifacts)
            report['blocked'] += [{'id': 'SECRET-UNTRIAGED', 'fingerprint': f['fingerprint']}
                                  for f in report['secrets']['findings'] if f['disposition'] == 'untriaged']
        if manifest:
            # Build-machine home paths, owner names and host names (MW-RELEASE-14).
            terms, allow = personal_paths.identity_terms(), personal_paths.load_allow()
            found = personal_paths.scan([payload], terms, allow, personal_paths.seven_zip())
            raw = personal_paths.scan(artifacts, terms, allow, containers=False)
            report['personal_paths'] = {'files': found['files'] + raw['files'], 'containers': found['containers'],
                                        'total': found['total'] + raw['total'],
                                        'findings': found['findings'] + raw['findings']}
            report['blocked'] += [{'id': 'PERSONAL-PATH', 'file': f['file'], 'category': f['category'],
                                   'count': f['count']} for f in report['personal_paths']['findings']]
        if manifest:
            release_input(manifest_path, source['commit'])
            if digest(manifest_path) != report['manifest_sha256']:
                raise ScanError('release manifest changed during scan')
        if source_receipt(ROOT) != source:
            raise ScanError('source changed during scan')
        report['status'] = 'blocked' if report['blocked'] else ('inventory-only' if args.command == 'inventory' else 'passed')
        write(Path(args.out), report)
        if args.sbom:
            write(Path(args.sbom), report['sbom'])
        print(f"Security {args.command}: {report['status']}; {len(entries)} components; "
              f"{len(report['findings'])} findings; {len(report['blocked'])} blockers. Receipt written.")
        return 1 if report['blocked'] else 0
    except Exception as error:
        # Unexpected parser/IO/network/tool errors are not clean scans. Never
        # echo an exception containing scanner output, URLs, file contents, etc.
        report['status'] = 'incomplete'
        report['error'] = str(error) if isinstance(error, ScanError) else 'invalid or unavailable scan input (details withheld)'
        write(Path(args.out), report)
        print('Security gate incomplete: ' + report['error'], file=sys.stderr)
        return 2


def verify(args):
    if not args.receipt or not args.manifest:
        raise ScanError('verify requires --receipt and --manifest')
    receipt = json.loads(Path(args.receipt).read_text())
    source = source_receipt(ROOT)
    if receipt.get('kind') != 'release' or receipt.get('status') != 'passed' or receipt.get('source') != source or source['dirty']:
        raise ScanError('receipt must be a passed release for this exact clean source')
    queried = dt.datetime.fromisoformat(receipt['advisory_database']['queried_at'])
    age = dt.datetime.now(dt.timezone.utc) - queried
    if age.total_seconds() < 0 or age > dt.timedelta(hours=24):
        raise ScanError('advisory receipt older than 24 hours or in the future; rescan')
    manifest = Path(args.manifest).resolve()
    if digest(manifest) != receipt.get('manifest_sha256'):
        raise ScanError('manifest no longer matches receipt')
    data, _, _, _ = release_input(manifest, source['commit'])
    if data != receipt.get('artifact_evidence'):
        raise ScanError('artifact evidence no longer matches receipt')
    policy = json.loads((ROOT / 'scripts/security/policy.json').read_text())
    if policy_findings(receipt['findings'], policy, data['platform']):
        raise ScanError('release findings no longer satisfy policy')
    if any(f['disposition'] == 'untriaged' for f in receipt['secrets']['findings']):
        raise ScanError('release has untriaged secrets')
    print('Security release receipt verified for the current source, artifacts, policy and freshness window.')
    return 0


def run(args):
    try:
        return verify(args) if args.command == 'verify' else scan(args)
    except Exception as error:
        message = str(error) if isinstance(error, ScanError) else 'invalid or unavailable scan input (details withheld)'
        if args.command != 'verify':
            write(Path(args.out), {'schema_version': 1, 'status': 'incomplete', 'error': message})
        print('Security gate incomplete: ' + message, file=sys.stderr)
        return 2


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('inventory', 'scan', 'secrets', 'release', 'verify'))
    parser.add_argument('--ecosystem', choices=('all', 'cargo', 'pnpm', 'maven', 'native'), default='all')
    parser.add_argument('--platform', choices=('all', 'windows', 'macos', 'android', 'web'), default='all')
    parser.add_argument('--manifest', help='release manifest binding artifacts, extracted payload, source and native components')
    parser.add_argument('--out', default='.local/security/receipt.json')
    parser.add_argument('--receipt', help='passed release receipt to verify before publication')
    parser.add_argument('--sbom', help='also write standalone CycloneDX 1.6 JSON')
    args = parser.parse_args()
    outputs = [Path(args.out).resolve()] + ([Path(args.sbom).resolve()] if args.sbom else [])
    if len(outputs) != len(set(outputs)):
        parser.error('receipt and SBOM outputs must be different files')
    if args.manifest:
        manifest = Path(args.manifest).resolve()
        try:
            data = json.loads(manifest.read_text())
            payload = (manifest.parent / data['payload']).resolve()
            inputs = [manifest] + [(manifest.parent / item['path']).resolve() for item in data['artifacts']]
            if any(output in inputs or output.is_relative_to(payload) for output in outputs):
                parser.error('outputs must be outside the manifest, artifacts and payload')
        except (OSError, ValueError, KeyError, TypeError):
            pass  # The scan writes an incomplete receipt for malformed inputs.
    if args.command == 'release' and args.ecosystem != 'all':
        parser.error('release must scan every ecosystem applicable to its platform')
    return run(args)


if __name__ == '__main__':
    sys.exit(main())
