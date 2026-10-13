#!/usr/bin/env python3
"""Refuse release artifacts that carry build-machine home paths or owner identity.

Every file of the unpacked artifact is read as bytes, including binaries. ASCII
and UTF-16LE strings are both searched. Nested zip/tar/gzip containers are
unpacked in a private temporary directory; NSIS installers and DMG images need
7-Zip (`--seven-zip` or `7z`/`7zz` on PATH), and a container that cannot be
unpacked fails the scan instead of being skipped.

Owner identity terms (user and account names, host names) are private and are
read from a file outside the repository (`--identity`, `WRITER_RELEASE_IDENTITY`
or ~/.config/tricktic/release-identity.json). The current user, home folder name
and host name are always added. Reports carry counts, file names and redacted
samples only: identity terms and home-folder names never appear in output.

Owner terms inside an embedded code signature pass: the signer's certificate
name is part of every signed release. Home paths there are still reported. The
tracked allow list (personal-paths.json) names other exact strings that may
contain a term, each with a reason; see docs/security/release-personal-paths.md.
"""
import argparse
import getpass
import gzip
import json
import os
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile

from scanning import ScanError

HERE = Path(__file__).resolve().parent
ALLOW_FILE = HERE / 'personal-paths.json'
DEFAULT_IDENTITY = Path('~/.config/tricktic/release-identity.json')
MAX_DEPTH = 4
MAX_UNPACKED = 4 * 1024**3

# Home folders on each platform, with the folder name captured. `\\\\` covers
# backslashes escaped in source strings and JSON. A placeholder segment such as
# `<user>` or `$USER` is not a real folder and does not match.
HOME = [
    ('home-path', re.compile(rb'/home/([A-Za-z0-9._-]+)')),
    ('home-path', re.compile(rb'/Users/([A-Za-z0-9._-]+)')),
    ('home-path', re.compile(rb'(?i)\b[A-Z]:(?:\\\\|\\|/)(?:Users|Documents and Settings)(?:\\\\|\\|/)([A-Za-z0-9._ -]+)')),
]
# Shared, non-personal folders under the home roots.
SYSTEM_HOMES = {b'shared', b'public', b'default', b'default user', b'all users'}
# Shared service accounts; never a person's name and too common to search for.
GENERIC_ACCOUNTS = {'root', 'user', 'admin', 'administrator', 'runner', 'builder', 'build', 'ubuntu',
                    'localhost', 'vagrant', 'docker', 'ci'}
WIDE = re.compile(rb'(?:[\x20-\x7e]\x00){4,}')


def load_allow(path=ALLOW_FILE):
    data = json.loads(Path(path).read_text())
    allow = []
    for item in data.get('allow', []):
        if not item.get('text') or not item.get('reason') or item.get('scope', 'any') not in ('any', 'code-signature'):
            raise ScanError('each allow entry needs text, reason and scope any|code-signature')
        allow.append({'text': item['text'].encode(), 'scope': item.get('scope', 'any')})
    return allow


def identity_terms(path=None, env=os.environ):
    """Private owner terms plus this host's user, home and host names."""
    selected = path or env.get('WRITER_RELEASE_IDENTITY') or DEFAULT_IDENTITY
    file = Path(selected).expanduser()
    if not file.is_file():
        raise ScanError('owner identity file missing; create it outside the repository '
                        '(docs/security/release-personal-paths.md)')
    data = json.loads(file.read_text())
    if data.get('schema_version') != 1 or not isinstance(data.get('terms'), list) or not data['terms']:
        raise ScanError('identity file needs schema_version 1 and a non-empty terms list')
    terms = set(data['terms'])
    host = socket.gethostname()
    automatic = {getpass.getuser(), Path.home().name, host, host.split('.')[0]}
    terms.update(t for t in automatic if t.lower() not in GENERIC_ACCOUNTS)
    terms = {t.strip() for t in terms if isinstance(t, str) and len(t.strip()) >= 3}
    return sorted(terms, key=lambda t: (-len(t), t.lower()))


def term_pattern(terms):
    # Alphanumeric boundaries: `alex` matches `alex-pc` and `/alex/`, not `alexa`.
    escaped = b'|'.join(re.escape(t.encode()) for t in terms)
    return re.compile(rb'(?i)(?<![A-Za-z0-9])(?:' + escaped + rb')(?![A-Za-z0-9])')


def signature_ranges(raw):
    """Byte ranges of embedded code signatures (Mach-O, fat Mach-O, PE Authenticode, DMG)."""
    ranges = []
    try:
        if len(raw) >= 512 and raw[-512:-508] == b'koly':
            # UDIF trailer: CodeSignatureOffset and CodeSignatureLength.
            offset, length = struct.unpack_from('>QQ', raw, len(raw) - 512 + 296)
            if offset and length and offset + length <= len(raw):
                ranges.append((offset, offset + length))
        if raw[:4] in (b'\xca\xfe\xba\xbe', b'\xca\xfe\xba\xbf') and len(raw) >= 8:
            count = struct.unpack_from('>I', raw, 4)[0]
            wide = raw[:4] == b'\xca\xfe\xba\xbf'
            if count < 64:
                for index in range(count):
                    if wide:
                        offset, size = struct.unpack_from('>QQ', raw, 8 + index*32 + 8)
                    else:
                        offset, size = struct.unpack_from('>II', raw, 8 + index*20 + 8)
                    ranges += [(offset + a, offset + b) for a, b in signature_ranges(raw[offset:offset+size])]
        elif raw[:4] == b'\xcf\xfa\xed\xfe':
            commands, cursor = struct.unpack_from('<I', raw, 16)[0], 32
            for _ in range(min(commands, 4096)):
                command, size = struct.unpack_from('<II', raw, cursor)
                if command == 0x1d:
                    offset, length = struct.unpack_from('<II', raw, cursor + 8)
                    ranges.append((offset, offset + length))
                cursor += max(size, 8)
        elif raw[:2] == b'MZ':
            pe = struct.unpack_from('<I', raw, 0x3c)[0]
            if raw[pe:pe+4] == b'PE\x00\x00':
                magic = struct.unpack_from('<H', raw, pe + 24)[0]
                directory = pe + 24 + (128 if magic == 0x20b else 112) + 4*8
                offset, length = struct.unpack_from('<II', raw, directory)
                if offset and length:
                    ranges.append((offset, offset + length))
    except struct.error:
        pass
    return ranges


def views(raw):
    """(text, map) pairs: the raw bytes, then UTF-16LE strings with original offsets."""
    yield raw, None
    pieces, spans, cursor = [], [], 0
    for match in WIDE.finditer(raw):
        text = match.group()[::2]
        spans.append((cursor, cursor + len(text), match.start()))
        pieces.append(text + b'\n')
        cursor += len(text) + 1
    if pieces:
        yield b''.join(pieces), spans


def original(offset, spans):
    if spans is None:
        return offset
    for start, end, source in spans:
        if start <= offset < end:
            return source + 2*(offset - start)
    return offset


def allowed(lowered, start, end, allow, signature, spans):
    for item in allow:
        needle = item['text'].lower()
        found = lowered.find(needle, max(0, end - len(needle)), start + len(needle))
        while found != -1 and found <= start:
            if found + len(needle) >= end:
                position = original(found, spans)
                if item['scope'] == 'any' or any(a <= position < b for a, b in signature):
                    return True
            found = lowered.find(needle, found + 1, start + len(needle))
    return False


def redact(sample, terms):
    for category, pattern in HOME:
        sample = pattern.sub(lambda m: m.group(0)[:m.start(1)-m.start(0)] + b'<home>', sample)
    if terms:
        sample = term_pattern(terms).sub(b'<owner>', sample)
    return sample.decode('ascii', 'replace')


def scan_bytes(raw, terms, allow, name=''):
    """Return {category: (count, first redacted sample)} for one buffer."""
    found = {}
    signature = signature_ranges(raw)
    owner = term_pattern(terms) if terms else None
    for text, spans in views(raw):
        hits = []
        for category, pattern in HOME:
            for match in pattern.finditer(text):
                if match.group(1).lower().strip() in SYSTEM_HOMES:
                    continue
                hits.append((category, match.start(), match.end()))
        if owner:
            # A term inside a reported home path is the same occurrence.
            paths = [(start, end) for _, start, end in hits]
            hits += [('owner-term', m.start(), m.end()) for m in owner.finditer(text)
                     if not any(a <= m.start() < b for a, b in paths)]
        lowered = text.lower() if hits else b''
        for category, start, end in hits:
            # A signed release carries the signer's certificate name; it needs no tracked entry.
            if category == 'owner-term' and any(a <= original(start, spans) < b for a, b in signature):
                continue
            if allowed(lowered, start, end, allow, signature, spans):
                continue
            line_start = max(text.rfind(b'\x00', 0, start), text.rfind(b'\n', 0, start), start - 80) + 1
            line_end = min([p for p in (text.find(b'\x00', end), text.find(b'\n', end)) if p != -1] + [end + 80])
            count, sample = found.get(category, (0, None))
            sample = sample or redact(re.sub(rb'[^\x20-\x7e]', b'.', text[line_start:line_end]), terms)
            found[category] = (count + 1, sample)
    for category, start, end in [(c, m.start(), m.end()) for c, p in HOME for m in p.finditer(name.encode())] + (
            [('owner-term', m.start(), m.end()) for m in owner.finditer(name.encode())] if owner else []):
        count, sample = found.get('file-name', (0, None))
        found['file-name'] = (count + 1, sample or redact(name.encode(), terms))
    return found


def seven_zip(explicit=None):
    for candidate in [explicit, os.environ.get('WRITER_SEVEN_ZIP'), shutil.which('7zz'), shutil.which('7z')]:
        if candidate and Path(candidate).is_file():
            return candidate
    return None


def container(path):
    with path.open('rb') as stream:
        head = stream.read(512)
    if head[:4] == b'PK\x03\x04':
        return 'zip'
    if head[:2] == b'\x1f\x8b':
        return 'gzip'
    if len(head) >= 262 and head[257:262] == b'ustar':
        return 'tar'
    if head[:2] == b'MZ':
        with path.open('rb') as stream:
            if b'NullsoftInst' in stream.read(4 * 1024 * 1024):
                return 'nsis'
    if path.suffix.lower() == '.dmg':
        return 'dmg'
    return None


def safe_member(name):
    parts = Path(name).parts
    return parts and not Path(name).is_absolute() and '..' not in parts


def unpack(path, kind, destination, tool):
    destination.mkdir(parents=True)
    if kind == 'zip':
        with zipfile.ZipFile(path) as archive:
            if sum(i.file_size for i in archive.infolist()) > MAX_UNPACKED:
                raise ScanError('archive too large to unpack')
            for info in archive.infolist():
                if not safe_member(info.filename):
                    raise ScanError('unsafe archive member path')
            archive.extractall(destination)
    elif kind in ('tar', 'gzip'):
        try:
            with tarfile.open(path) as archive:
                archive.extractall(destination, filter='data')
        except tarfile.ReadError:
            if kind != 'gzip':
                raise
            with gzip.open(path) as stream, (destination / path.stem).open('wb') as output:
                shutil.copyfileobj(stream, output)
    else:
        if not tool:
            raise ScanError(f'{kind} container needs 7-Zip; pass --seven-zip')
        source, target = str(path), str(destination)
        if tool.lower().endswith('.exe') and shutil.which('wslpath'):
            # WSL build hosts use the Windows 7-Zip (26.03+ reads NSIS 3.12).
            source, target = (subprocess.check_output(['wslpath', '-w', p], text=True).strip() for p in (source, target))
        result = subprocess.run([tool, 'x', '-y', '-bso0', '-bsp0', f'-o{target}', source],
                                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        # A DMG's /Applications shortcut is a link 7-Zip refuses to create; links
        # carry no file content. Any other error fails the scan.
        errors = [line for line in result.stderr.decode(errors='replace').splitlines() if line.strip()]
        links_only = errors and all(line.startswith('ERROR: Dangerous link path was ignored') for line in errors)
        if result.returncode not in (0, 1) and not (result.returncode == 2 and links_only):
            raise ScanError(f'7-Zip could not unpack a {kind} container')


def walk(root, label, terms, allow, tool, work, depth, report, containers=True):
    files = sorted(p for p in root.rglob('*') if p.is_file() and not p.is_symlink()) if root.is_dir() else [root]
    for file in files:
        relative = label + ('/' + file.relative_to(root).as_posix() if root.is_dir() else '')
        report['files'] += 1
        found = scan_bytes(file.read_bytes(), terms, allow, relative)
        for category, (count, sample) in found.items():
            report['findings'].append({'file': redact(relative.encode(), terms), 'category': category,
                                       'count': count, 'sample': sample})
        kind = containers and container(file)
        if kind:
            if depth >= MAX_DEPTH:
                raise ScanError('containers nested too deeply')
            target = Path(tempfile.mkdtemp(dir=work))
            unpack(file, kind, target / 'x', tool)
            report['containers'] += 1
            walk(target / 'x', relative + '!', terms, allow, tool, work, depth + 1, report)
            shutil.rmtree(target)


def scan(paths, terms, allow, tool=None, containers=True):
    """Scan files/directories (and their nested containers). Never prints matches.

    `containers=False` reads final artifacts as bytes only, when the release
    manifest already binds their fully extracted payload."""
    report = {'files': 0, 'containers': 0, 'findings': []}
    with tempfile.TemporaryDirectory(prefix='writer-personal-') as work:
        for path in paths:
            path = Path(path)
            if not path.exists():
                raise ScanError('scan input missing')
            walk(path, path.name, terms, allow, tool, work, 0, report, containers)
    report['total'] = sum(f['count'] for f in report['findings'])
    report['status'] = 'blocked' if report['findings'] else 'passed'
    return report


def summary(report, limit=40):
    lines = [f"Personal-path scan: {report['status']}; {report['files']} files "
             f"({report['containers']} containers unpacked); {report['total']} occurrences."]
    totals = {}
    for item in report['findings']:
        totals[item['category']] = totals.get(item['category'], 0) + item['count']
    lines += [f'  {category}: {count}' for category, count in sorted(totals.items())]
    for item in sorted(report['findings'], key=lambda f: -f['count'])[:limit]:
        lines.append(f"  {item['count']:6d} {item['category']:<10} {item['file']}  e.g. {item['sample'][:140]}")
    return '\n'.join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('artifact', nargs='+', type=Path, help='installer, archive, app bundle or unpacked directory')
    parser.add_argument('--identity', type=Path, help='private owner identity terms (outside the repository)')
    parser.add_argument('--seven-zip', help='7-Zip executable for NSIS/DMG containers')
    parser.add_argument('--out', type=Path, help='write the redacted JSON report here')
    args = parser.parse_args(argv)
    try:
        report = scan(args.artifact, identity_terms(args.identity), load_allow(), seven_zip(args.seven_zip))
    except (ScanError, OSError, ValueError, tarfile.TarError, zipfile.BadZipFile) as error:
        message = str(error) if isinstance(error, ScanError) else 'invalid or unreadable scan input (details withheld)'
        print('Personal-path scan incomplete: ' + message, file=sys.stderr)
        return 2
    if args.out:
        args.out.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
    print(summary(report))
    return 1 if report['findings'] else 0


if __name__ == '__main__':
    sys.exit(main())
