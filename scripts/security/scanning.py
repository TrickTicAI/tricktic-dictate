"""Fail-closed online advisories and redacted local secret scanning."""
import concurrent.futures
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.request
import collections
import tarfile
import zipfile
import stat

from public_metadata import public_findings, resource_regions, signature_regions


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


class ScanError(Exception):
    """Only fixed, non-sensitive messages may cross the CLI boundary."""


def command(args, cwd, allowed=(0,), timeout=300):
    try:
        result = subprocess.run(args, cwd=cwd, capture_output=True, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired):
        raise ScanError('required command unavailable or timed out') from None
    if result.returncode not in allowed:
        # Neither argv nor stderr: either can contain a credential or match.
        raise ScanError('required command failed (raw output withheld)')
    return result


def request(path, data=None):
    url = 'https://api.osv.dev/v1/' + path
    body = json.dumps(data).encode() if data is not None else None
    try:
        req = urllib.request.Request(url, data=body, headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=60) as response:
            return json.load(response)
    except Exception:
        raise ScanError('OSV request failed; no clean receipt issued') from None


def severity(advisory):
    # Unknown severity blocks too; do not guess a CVSS score or silently downrank.
    value = advisory.get('database_specific', {}).get('severity', 'unknown').lower()
    return {'medium': 'moderate'}.get(value, value) if value in {
        'low', 'medium', 'moderate', 'high', 'critical'} else 'unknown'


def osv(entries, fetch=request):
    started = now()
    queries, selected = [], []
    for item in entries:
        if item['ecosystem'] == 'native' and not item.get('osv_query'):
            continue
        queries.append(item.get('osv_query') or {'package': {'ecosystem': item['ecosystem'],
                                                           'name': item['name']}, 'version': item['version']})
        selected.append(item)
    matches = []
    for offset in range(0, len(queries), 100):
        pending = [(i, dict(q)) for i, q in enumerate(queries[offset:offset+100], offset)]
        pages = 0
        while pending:
            pages += 1
            if pages > 100:
                raise ScanError('OSV pagination limit reached')
            response = fetch('querybatch', {'queries': [q for _, q in pending]})
            results = response.get('results')
            if not isinstance(results, list) or len(results) != len(pending):
                raise ScanError('incomplete OSV batch response')
            follow = []
            for (i, query), result in zip(pending, results):
                if not isinstance(result, dict) or 'error' in result:
                    raise ScanError('invalid OSV result')
                for match in result.get('vulns', []):
                    if not re.fullmatch(r'[A-Za-z0-9._-]+', match.get('id', '')) or not match.get('modified'):
                        raise ScanError('invalid OSV advisory identity')
                    matches.append((i, match))
                if result.get('next_page_token'):
                    follow.append((i, {**query, 'page_token': result['next_page_token']}))
            pending = follow
    ids = sorted({m['id'] for _, m in matches})
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        records = dict(zip(ids, pool.map(lambda aid: fetch('vulns/' + aid), ids)))
    findings = []
    for i, match in matches:
        advisory = records[match['id']]
        if advisory.get('id') != match['id'] or not advisory.get('modified'):
            raise ScanError('incomplete OSV advisory record')
        if advisory.get('withdrawn'):
            continue
        item = selected[i]
        findings.append({'id': match['id'], 'purl': item['purl'], 'scopes': item['scopes'],
                         'severity': severity(advisory), 'reachability': 'unreviewed',
                         'modified': advisory['modified'], 'published': advisory.get('published'),
                         'aliases': advisory.get('aliases', []), 'evidence': item['evidence']})
    unique = {(f['id'], f['purl']): f for f in findings}
    return sorted(unique.values(), key=lambda f: (f['id'], f['purl'])), {
        'provider': 'https://api.osv.dev/v1/', 'queried_at': started,
        'completed_at': now(), 'packages_queried': len(queries), 'advisory_records': len(ids),
        'snapshot': 'live API; no global immutable database snapshot is exposed',
        'record_modified_at': {aid: records[aid]['modified'] for aid in ids}}


def policy_findings(findings, policy, platform, today=None):
    today = today or dt.datetime.now(dt.timezone.utc).date()
    if policy.get('schema_version') != 1:
        raise ScanError('unsupported security policy')
    exceptions = {}
    for exception in policy['exceptions']:
        fields = ('id', 'purl', 'platform', 'modified', 'expires', 'reason', 'reviewer', 'disposition')
        if any(not isinstance(exception.get(k), str) or not exception[k].strip() for k in fields):
            raise ScanError('incomplete security exception')
        if exception['disposition'] not in ('not-reachable', 'accepted-risk') or any(
                '*' in exception[k] for k in ('id', 'purl', 'platform', 'modified')):
            raise ScanError('security exceptions must be exact reviewed dispositions')
        try:
            expires = dt.date.fromisoformat(exception['expires'])
        except ValueError:
            raise ScanError('invalid exception expiry') from None
        if expires <= today:
            raise ScanError('expired security exception')
        key = tuple(exception[k] for k in ('id', 'purl', 'platform', 'modified'))
        if key in exceptions:
            raise ScanError('duplicate security exception')
        exceptions[key] = exception
    blocked = []
    for finding in findings:
        key = (finding['id'], finding['purl'], platform, finding['modified'])
        exception = exceptions.get(key)
        if exception:
            finding['disposition'] = {k: exception[k] for k in ('disposition', 'reviewer', 'reason', 'expires')}
        elif finding['severity'] in ('high', 'critical', 'unknown'):
            blocked.append({'id': finding['id'], 'purl': finding['purl']})
    return blocked


def sanitized_secrets(rows, approved, scope, reviewed=frozenset(), public=frozenset()):
    """`reviewed` holds payload fingerprints whose containing file hash was verified."""
    result = []
    for row in rows:
        fingerprint = row.get('Fingerprint', '')
        # No matched line, secret, description, author/email, message or entropy.
        disposition = 'untriaged'
        if scope == 'history' and fingerprint in approved:
            disposition = 'known-synthetic'
        elif scope.startswith('payload') and fingerprint in reviewed:
            disposition = 'reviewed-payload'
        elif scope == 'artifact' and fingerprint in reviewed:
            disposition = 'reviewed-payload-copy'
        elif scope.startswith('payload') and fingerprint in public:
            disposition = 'verified-public-signing-hash'
        result.append({'rule': row['RuleID'], 'file': row['File'], 'line': row['StartLine'],
                       'commit': row.get('Commit', ''), 'fingerprint': fingerprint,
                       'scope': scope, 'disposition': disposition})
    return result


def payload_allowances(policy):
    """Exact payload matches: payload-relative location, rule, line and file hash.

    The host staging path never enters a fingerprint, so a reviewed match stays
    valid across builds only while the containing payload file is byte-identical.
    """
    allowed = {}
    for item in policy.get('payload_secret_allowances', []):
        fingerprint, digest = item.get('fingerprint', ''), item.get('sha256', '')
        location = fingerprint.rsplit(':', 2)[0]
        if (not item.get('reason') or not re.fullmatch(r'[0-9a-f]{64}', digest)
                or not re.fullmatch(r'[A-Za-z0-9$._/ ()!+-]+:[a-z0-9-]+:[0-9]+', fingerprint)
                or location.startswith('/') or '..' in re.split(r'[/!]', location)
                or fingerprint in allowed):
            raise ScanError('payload secret allowance must name one exact payload match and file hash')
        allowed[fingerprint] = digest
    return allowed


def payload_file(location):
    """The shipped file holding a match: archive members and string dumps map to their container."""
    return location.removesuffix(' (binary strings)').split('!', 1)[0]


def payload_relative(file, payload, cwd):
    outer, bang, inner = file.partition('!')
    try:
        relative = (Path(cwd) / outer).resolve().relative_to(Path(cwd, payload).resolve()).as_posix()
    except ValueError:
        raise ScanError('payload secret finding outside the payload') from None
    return relative + bang + inner


def binary_strings(source, destination):
    """Expose ASCII and UTF-16 strings in binaries to the text secret scanner.

    Overlap covers tokens split at chunk boundaries. Temporary strings are never
    logged or retained; no external binary is executed or loaded.
    """
    patterns = (re.compile(rb'[\x20-\x7e]{8,}'),
                re.compile(rb'(?:[\x20-\x7e]\x00){8,}'),
                re.compile(rb'(?:\x00[\x20-\x7e]){8,}'))
    spans = []
    with source.open('rb') as stream, destination.open('wb') as output:
        carry, source_offset, output_offset = b'', 0, 0
        while chunk := stream.read(1024 * 1024):
            data = carry + chunk
            base = source_offset - len(carry)
            for index, pattern in enumerate(patterns):
                for match in pattern.finditer(data):
                    text = match.group().replace(b'\x00', b'')
                    spans.append((output_offset, output_offset + len(text),
                                  base + match.start() + (1 if index == 2 else 0),
                                  1 if index == 0 else 2))
                    output.write(text + b'\n')
                    output_offset += len(text) + 1
            carry = data[-65536:]
            source_offset += len(chunk)
    return spans


def artifact_payload_copy(row, root, payload, artifacts, allowances):
    """Adopt one exact reviewed payload match only for a verified archive copy."""
    outer, separator, member = row['File'].partition('!')
    path = (root / outer).resolve()
    if not separator or path not in {(root / p).resolve() for p in artifacts}:
        return False
    prefix = payload.name + '/'
    relative = member.removeprefix(prefix)
    if relative.startswith('/') or '..' in relative.split('/'):
        return False
    fingerprint = relative + ':' + row['RuleID'] + ':' + str(row['StartLine'])
    expected = allowances.get(fingerprint)
    if not expected:
        return False
    container = payload_file(relative)
    shipped = payload / container
    if not shipped.is_file() or hashlib.sha256(shipped.read_bytes()).hexdigest() != expected:
        return False
    archive_member = member.split('!', 1)[0]
    try:
        if zipfile.is_zipfile(path):
            with zipfile.ZipFile(path) as archive:
                if sum(x.filename == archive_member for x in archive.infolist()) != 1:
                    return False
                info = archive.getinfo(archive_member)
                if info.is_dir() or info.file_size > 64*1024*1024 or stat.S_ISLNK(info.external_attr >> 16):
                    return False
                copied = archive.read(info)
        else:
            with tarfile.open(path) as archive:
                if sum(x.name == archive_member for x in archive.getmembers()) != 1:
                    return False
                info = archive.getmember(archive_member)
                if not info.isfile() or info.size > 64*1024*1024:
                    return False
                copied = archive.extractfile(info).read()
        return copied == shipped.read_bytes()
    except (OSError, ValueError, KeyError, tarfile.TarError, zipfile.BadZipFile):
        return False


def secrets(root, policy, payload=None, artifacts=()):
    version = command(['gitleaks', 'version'], root).stdout.decode().strip()
    if not re.fullmatch(r'8\.30\.1', version):
        raise ScanError('gitleaks 8.30.1 required; review scanner changes before updating')
    if command(['git', 'rev-parse', '--is-shallow-repository'], root).stdout.strip() != b'false':
        raise ScanError('full Git history required for secret scan')
    approved = set()
    for item in policy.get('secret_fixtures', []):
        if not item.get('reason') or not re.fullmatch(r'[0-9a-f]{40}:[^\n]+:[a-z0-9-]+:[0-9]+', item.get('fingerprint', '')):
            raise ScanError('secret fixture must name one exact historical fingerprint')
        approved.add(item['fingerprint'])
    allowances = payload_allowances(policy)
    findings = []
    metadata = {}
    binary_context = {}
    def regions(file):
        if file not in metadata:
            raw = file.read_bytes()
            metadata[file] = (resource_regions(raw, payload, file)
                              if file.name == 'CodeResources' and file.parent.name == '_CodeSignature'
                              else signature_regions(raw))
        return metadata[file]
    with tempfile.TemporaryDirectory(prefix='writer-secret-') as temp:
        temp = Path(temp)
        # Explicit config and ignore path defeat repository/user bypasses.
        config = temp / 'gitleaks.toml'
        config.write_text('[extend]\nuseDefault = true\n')
        ignore = temp / 'ignore'
        ignore.write_text('')
        env_removed = {k: os.environ.pop(k) for k in list(os.environ) if k.startswith('GITLEAKS_')}
        try:
            scans = [('history', ['git', str(root), '--log-opts=--all --full-history'])]
            if payload:
                scans.append(('payload', ['dir', str(payload)]))
                strings = temp / 'binary-strings'
                strings.mkdir()
                # Scan every payload file: native magic and extension alone do
                # not identify all binary containers (DEX, packed resources).
                for index, file in enumerate(sorted(payload.rglob('*'))):
                    if file.is_file():
                        target = strings / f'{index}.txt'
                        binary_context[file] = (target, binary_strings(file, target))
                scans.append(('payload-binary-strings', ['dir', str(strings)]))
            for path in artifacts:
                scans.append(('artifact', ['dir', str(path)]))
            for scope, args in scans:
                report = temp / 'report.json'
                report.unlink(missing_ok=True)
                result = command(['gitleaks', *args, '--config', str(config), '--gitleaks-ignore-path', str(ignore),
                                  '--ignore-gitleaks-allow', '--redact=100', '--no-banner',
                                  '--max-archive-depth=8', '--max-decode-depth=5', '--exit-code=42',
                                  '--report-format=json', '--report-path', str(report)], root, allowed=(0, 42), timeout=900)
                for line in result.stderr.splitlines():
                    if re.search(rb'\b(ERR|WRN)\b', line) and b'leaks found' not in line.lower():
                        raise ScanError('secret scanner reported incomplete coverage (raw output withheld)')
                if not report.exists():
                    raise ScanError('secret scanner produced no report')
                rows = json.loads(report.read_text())
                if not isinstance(rows, list) or (result.returncode == 42 and not rows):
                    raise ScanError('invalid secret scanner report')
                if scope == 'payload-binary-strings':
                    paths = list(sorted(payload.rglob('*')))
                    for row in rows:
                        index = int(Path(row['File']).stem)
                        row['File'] = paths[index].relative_to(payload).as_posix() + ' (binary strings)'
                        row['Fingerprint'] = row['File'] + ':' + row['RuleID'] + ':' + str(row['StartLine'])
                elif scope == 'payload':
                    for row in rows:
                        row['File'] = payload_relative(row['File'], payload, root)
                        row['Fingerprint'] = row['File'] + ':' + row['RuleID'] + ':' + str(row['StartLine'])
                reviewed, public = set(), set()
                for row in rows if scope.startswith('payload') else ():
                    expected = allowances.get(row['Fingerprint'])
                    shipped = payload / payload_file(row['File'])
                    if expected and shipped.is_file() and hashlib.sha256(shipped.read_bytes()).hexdigest() == expected:
                        reviewed.add(row['Fingerprint'])
                if scope.startswith('payload'):
                    grouped = collections.defaultdict(list)
                    for row in rows:
                        if '!' not in row['File']:
                            grouped[payload / payload_file(row['File'])].append(row)
                    for file, file_rows in grouped.items():
                        verified_regions = regions(file)
                        if not verified_regions:
                            continue
                        if scope == 'payload-binary-strings':
                            scanned, spans = binary_context[file]
                            public.update(public_findings(file_rows, scanned.read_bytes(), verified_regions, spans))
                        else:
                            public.update(public_findings(file_rows, file.read_bytes(), verified_regions))
                elif scope == 'artifact' and payload:
                    for row in rows:
                        if artifact_payload_copy(row, root, payload, artifacts, allowances):
                            reviewed.add(row['Fingerprint'])
                findings.extend(sanitized_secrets(rows, approved, scope, reviewed, public))
        finally:
            os.environ.update(env_removed)
    return {'scanner': 'gitleaks', 'version': version, 'scanned_at': now(),
            'history': 'all local refs; full history required', 'findings': findings}
