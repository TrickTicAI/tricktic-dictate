#!/usr/bin/env python3
"""Offline, redacted review of MW-SEC-29's retained artifact secret locations.

This reads the original blocked receipt, signed payload and independently fetched
public-source files. It never changes policy, reruns a gate, executes an artifact
or emits a matched value. Output is supporting evidence, not release approval.
"""
import argparse
import collections
import datetime
import hashlib
import json
from pathlib import Path
import plistlib
import re
import struct
import tarfile

ORIGINAL_RECEIPT = 'b8d18deab6c3b20ceac0ca16f4541f1142d4f7c99966723a6412e6e1bceeaae1'
CANDIDATE = '12ca1e06b420888036d6f68a4d8cb63c8252a35c'


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def strings_at(raw, wanted):
    # Match the retained scanner's ASCII/UTF-16 ordering and chunk overlap.
    patterns = (re.compile(rb'[\x20-\x7e]{8,}'),
                re.compile(rb'(?:[\x20-\x7e]\x00){8,}'),
                re.compile(rb'(?:\x00[\x20-\x7e]){8,}'))
    found, number, carry = {}, 0, b''
    for offset in range(0, len(raw), 1024 * 1024):
        data = carry + raw[offset:offset + 1024 * 1024]
        for pattern in patterns:
            for match in pattern.finditer(data):
                number += 1
                if number in wanted:
                    found[number] = match.group().replace(b'\x00', b'')
        carry = data[-65536:]
    assert set(found) == set(wanted), 'Scanner location not reproduced'
    return found


def signature_metadata(raw):
    assert raw[:4] == b'\xcf\xfa\xed\xfe', 'Expected native Mach-O64'
    count = struct.unpack_from('<I', raw, 16)[0]
    cursor, signature = 32, None
    for _ in range(count):
        command, size = struct.unpack_from('<II', raw, cursor)
        assert size >= 8 and cursor + size <= len(raw)
        if command == 0x1d:
            offset, length = struct.unpack_from('<II', raw, cursor + 8)
            signature = raw[offset:offset + length]
            assert len(signature) == length
        cursor += size
    assert signature is not None
    magic, length, count = struct.unpack_from('>III', signature)
    assert magic == 0xfade0cc0 and length <= len(signature)
    hashes, cms = [], None
    for i in range(count):
        slot, offset = struct.unpack_from('>II', signature, 12 + 8*i)
        magic, size = struct.unpack_from('>II', signature, offset)
        blob = signature[offset:offset + size]
        assert len(blob) == size
        if magic == 0xfade0c02:
            kind = blob[37]
            hasher = {1: hashlib.sha1, 2: hashlib.sha256,
                      3: hashlib.sha256, 4: hashlib.sha384}[kind]
            hashes.append(hasher(blob).digest()[:20])
        elif slot == 0x10000:
            assert magic == 0xfade0b01
            cms = blob[8:]
    assert cms is not None and hashes
    declarations = []
    for match in re.finditer(rb'<\?xml.*?</plist>', cms, re.S):
        doc = plistlib.loads(match.group())
        if isinstance(doc, dict) and 'cdhashes' in doc:
            declarations.extend(doc['cdhashes'])
    assert declarations and sorted(declarations) == sorted(hashes)
    return len(hashes)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--receipt', required=True, type=Path)
    parser.add_argument('--public-sources', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[3]
    receipt_path = args.receipt.resolve(strict=True)
    raw_receipt = receipt_path.read_bytes()
    assert digest(raw_receipt) == ORIGINAL_RECEIPT, 'Requires retained original receipt'
    receipt = json.loads(raw_receipt)
    assert receipt['status'] == 'blocked' and receipt['source']['commit'] == CANDIDATE
    gate, source_root = receipt_path.parent, receipt_path.parents[2]
    assert source_root == (repo / '.local/acceptance/gate-source').resolve(), 'Requires this worktree retained source'
    evidence = receipt['artifact_evidence']
    payload = (gate / evidence['payload']).resolve(strict=True)
    output = args.out.resolve()
    tracked_output = repo / 'docs/security/evidence/mw-sec-29/exact-artifact-secret-review.json'
    assert output == tracked_output or output.is_relative_to(repo / '.local/acceptance')
    assert not output.is_relative_to(source_root) and output != receipt_path
    policy = source_root / 'scripts/security/policy.json'
    policy_before = digest(policy.read_bytes())
    assert policy_before == receipt['source']['input_sha256']['scripts/security/policy.json']
    artifacts = {a['path']: a['sha256'] for a in evidence['artifacts']}
    for name, expected in artifacts.items():
        assert digest((gate / name).read_bytes()) == expected
        assert output != (gate / name).resolve()
    public = args.public_sources.resolve(strict=True)
    assert public.is_relative_to(repo / '.local/acceptance')
    assert not output.is_relative_to(public)
    upstream = json.loads((public / 'proof.json').read_text())
    assert upstream['nemo_cpu_and_metal_checksums_match_public_file']
    assert upstream['ffmpeg_archive_matches_signature_verified_pin']
    assert upstream['ffmpeg_retained_source_member_equals_public_source']
    api = (public / 'nemo-api.md').read_bytes()
    ffmpeg = (public / 'ffmpeg-vf_hsvkey.c').read_bytes()
    header = (source_root / 'runtime/whisper/writer-auth.h').read_bytes()
    auth_digest = digest(header)
    sources = {}
    for family, field in [('nemo', 'writer_auth_sha256'),
                          ('whisper', 'examples/server/writer-auth.h')]:
        text = (payload / f'Contents/Resources/notices/writer-{family}-source.txt').read_text()
        values = dict(line.split('=', 1) for line in text.splitlines() if '=' in line)
        assert values[field] == auth_digest
        assert all(re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', v) for v in values.values())
        sources[family] = text
        for backend in ['cpu', 'metal']:
            build = json.loads((payload / f'Contents/Resources/notices/{family}-{backend}-build.json').read_text())
            assert build['source'] == text
            if family == 'nemo':
                assert build['files']['share/doc/nemo-speech/docs/api.md'] == digest(api)
    seal_path = 'Contents/_CodeSignature/CodeResources'
    seal = plistlib.loads((payload / seal_path).read_bytes())
    references = 0
    for section in ['files', 'files2']:
        for relative, value in seal[section].items():
            path = (payload / 'Contents' / relative).resolve()
            assert path.is_relative_to(payload) and path.is_file()
            raw = path.read_bytes()
            if isinstance(value, bytes):
                assert value == hashlib.sha1(raw).digest()
                references += 1
            else:
                assert isinstance(value, dict)
                for key, hasher in [('hash', hashlib.sha1), ('hash2', hashlib.sha256)]:
                    if key in value:
                        assert value[key] == hasher(raw).digest()
                        references += 1
    groups = collections.defaultdict(list)
    for row in receipt['secrets']['findings']:
        if row['disposition'] == 'untriaged':
            groups[(row['scope'], row['file'])].append(row)
    entries, files = [], {}
    for (scope, location), rows in groups.items():
        relative = location.removesuffix(' (binary strings)')
        if scope == 'artifact':
            archive, relative = relative.split('!TrickTic Dictate.app/', 1)
            assert Path(archive).resolve() == (gate / 'candidate.tar.gz').resolve()
        container, separator, member = relative.partition('!')
        path = payload / container
        raw = path.read_bytes()
        expected = evidence['files'][container]['sha256']
        assert digest(raw) == expected
        files[container] = expected
        if scope == 'artifact':
            with tarfile.open(gate / 'candidate.tar.gz') as archive:
                archived = archive.extractfile('TrickTic Dictate.app/' + container).read()
            assert archived == raw, 'Archive member differs from exact payload'
        if scope == 'payload-binary-strings':
            lines = strings_at(raw, {row['line'] for row in rows})
        else:
            if separator:
                with tarfile.open(path) as archive:
                    raw = archive.extractfile(member).read()
                assert raw == ffmpeg
            split = raw.splitlines()
            lines = {row['line']: split[row['line'] - 1] for row in rows}
        for row in rows:
            line = lines[row['line']]
            classification, detail = 'unclassified', {}
            if container == seal_path:
                classification = 'public-resource-seal'
                detail['resource_hash_references_verified'] = references
            elif line == b'<key>cdhashes</key>':
                classification = 'public-code-directory-hash-metadata'
                detail['declared_code_directory_hashes_verified'] = signature_metadata(raw)
            elif separator and member == 'ffmpeg-8.0.3/libavfilter/vf_hsvkey.c':
                classification = 'unchanged-public-ffmpeg-source'
                detail['source_member_sha256'] = digest(ffmpeg)
            elif container.endswith('writer-nemo-source.txt') or container.endswith('writer-whisper-source.txt'):
                field, value = line.decode().split('=', 1)
                if field in ['writer_auth_sha256', 'examples/server/writer-auth.h'] and value == auth_digest:
                    classification = 'public-auth-header-source-checksum'
                    detail['header_matches_exact_build_source'] = True
            elif container.endswith(('-cpu-build.json', '-metal-build.json')):
                part = json.loads('{' + line.decode().strip().rstrip(',') + '}')
                if 'source' in part and part['source'] in sources.values():
                    classification = 'public-source-build-receipt'
                    detail['source_equals_shipped_source_notice'] = True
                elif part.get('share/doc/nemo-speech/docs/api.md') == digest(api):
                    classification = 'public-api-documentation-checksum'
                    detail['checksum_equals_exact_upstream_revision'] = True
            entries.append({'scope': scope, 'file': relative, 'line': row['line'],
                            'rule': row['rule'], 'fingerprint_sha256': digest(row['fingerprint'].encode()),
                            'container_sha256': expected, 'classification': classification,
                            'verification': detail, 'matched_values_retained': False})
    counts = dict(collections.Counter(row['classification'] for row in entries))
    assert len(entries) == 471
    assert digest(receipt_path.read_bytes()) == ORIGINAL_RECEIPT
    assert digest(policy.read_bytes()) == policy_before
    result = {'schema_version': 1, 'checked_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'kind': 'read-only proposed exact-artifact secret classification',
              'candidate_commit': CANDIDATE, 'original_receipt_sha256': ORIGINAL_RECEIPT,
              'review_fixture_sha256': digest(Path(__file__).read_bytes()),
              'original_gate_status': 'blocked', 'original_policy_sha256': policy_before,
              'original_blocker_count': len(receipt['blocked']), 'reviewed_locations': len(entries),
              'classification_counts': counts, 'unclassified_locations': counts.get('unclassified', 0),
              'public_sources': upstream, 'artifacts': artifacts, 'files': files, 'entries': entries,
              'policy_changed': False, 'gate_changed': False, 'matched_values_retained': False,
              'limits': ['Classification evidence is not an approved policy disposition or passing gate',
                         'Existing native strict-signature proof supplies CMS signature verification; this helper reproduces declared hash content',
                         'Advisory blockers and remaining native update/publisher acceptance are unchanged']}
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'reviewed_locations': len(entries), 'classification_counts': counts,
                      'unclassified_locations': counts.get('unclassified', 0), 'gate_changed': False}))


if __name__ == '__main__':
    main()
