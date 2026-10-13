"""Bind release receipts to the exact packaged files and reviewed source inventory."""
import json
from pathlib import Path
import re
from urllib.parse import urlsplit

from inventory import component, digest
from scanning import ScanError


PLATFORMS = ('windows', 'macos', 'android', 'web')


def child(root, value):
    path = Path(value)
    if path.is_absolute() or '..' in path.parts or not path.parts:
        raise ScanError('manifest paths must be relative and contained')
    full = root / path
    if not full.resolve().is_relative_to(root.resolve()) or any(
            parent.is_symlink() for parent in [full, *full.parents] if parent != root.parent):
        raise ScanError('manifest symlinks are not accepted; stage dereferenced payload')
    return full


def native_file(path):
    with path.open('rb') as stream:
        magic = stream.read(4)
    return magic[:2] == b'MZ' or magic == b'\x7fELF' or magic in (
        b'\xfe\xed\xfa\xce', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xcf\xfa\xed\xfe',
        b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca', b'\xca\xfe\xba\xbf') or bool(
        re.search(r'\.(exe|dll|dylib|so)(\.|$)', path.name, re.I))


def release_input(path, commit):
    data = json.loads(path.read_text())
    base = path.parent.resolve()
    if data.get('schema_version') != 1 or data.get('platform') not in PLATFORMS or data.get('source_commit') != commit:
        raise ScanError('release manifest must name this exact commit and a supported platform')
    if not data.get('builder') or not data.get('extraction_method'):
        raise ScanError('release manifest requires builder and artifact extraction evidence')
    payload = child(base, data['payload'])
    if not payload.is_dir():
        raise ScanError('release payload directory missing')
    actual = {}
    for file in sorted(payload.rglob('*')):
        if file.is_symlink():
            raise ScanError('release payload must have no symlinks')
        if file.is_file():
            actual[file.relative_to(payload).as_posix()] = {'sha256': digest(file), 'bytes': file.stat().st_size}
        elif not file.is_dir():
            raise ScanError('unsupported release file type')
    if not actual or data.get('files') != actual:
        raise ScanError('payload inventory differs from manifest (missing, extra, or changed files)')
    artifacts = []
    for item in data['artifacts']:
        file = child(base, item['path'])
        if not file.is_file() or digest(file) != item['sha256']:
            raise ScanError('artifact hash mismatch')
        artifacts.append(file)
    if not artifacts:
        raise ScanError('release requires at least one final distributable artifact')
    entries, covered = [], set()
    for item in data['components']:
        if not item.get('files') or not item.get('name') or not item.get('version'):
            raise ScanError('incomplete native component')
        if 'vendor' in item:
            # Closed-source redistributables (e.g. Microsoft runtime DLLs) have
            # no public source revision or OSV identity. They are inventoried
            # without osv_query, so the scan reports NATIVE-PROVENANCE-UNKNOWN
            # and blocks unless the owner records an exact expiring exception.
            vendor = item['vendor']
            if 'source' in item or 'osv_query' in item or not isinstance(vendor, dict) or set(vendor) != {
                    'publisher', 'authenticode_subject'} or not all(
                    isinstance(v, str) and v.strip() for v in vendor.values()):
                raise ScanError('vendor component requires only publisher and authenticode_subject')
            for filename in item['files']:
                if filename not in actual:
                    raise ScanError('component names file absent from payload')
                covered.add(filename)
            entries.append(component('native', item['name'], item['version'], ['production'],
                                     'release-manifest-vendor', platform=data['platform'], vendor=vendor,
                                     files=sorted(item['files'])))
            continue
        source = item['source']
        url = urlsplit(source['url'])
        if url.scheme != 'https' or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ScanError('native source must be a public HTTPS URL without credentials')
        if not re.fullmatch(r'[a-f0-9]{40,64}', source.get('revision', '')):
            raise ScanError('native source must have an exact revision')
        query = item['osv_query']
        if set(query) == {'commit'}:
            if query['commit'] != source['revision'] or not re.fullmatch(r'[a-f0-9]{40}', query['commit']):
                raise ScanError('native OSV commit must match source revision')
        elif set(query) == {'package', 'version'}:
            if not query['package'].get('name') or not query['package'].get('ecosystem') or query['version'] != item['version']:
                raise ScanError('native OSV package identity missing or inconsistent')
        else:
            raise ScanError('native component requires exact OSV package/version or source commit')
        for filename in item['files']:
            if filename not in actual:
                raise ScanError('component names file absent from payload')
            covered.add(filename)
        entries.append(component('native', item['name'], item['version'], ['production'],
                                 'release-manifest', platform=data['platform'], source=source,
                                 files=sorted(item['files']), osv_query=query))
    if any(native_file(payload / name) and name not in covered for name in actual):
        raise ScanError('native binary lacks component/source/advisory identity')
    # Catch replacement between validation and the completed scan as well.
    return data, payload, artifacts, sorted(entries, key=lambda item: item['purl'])
