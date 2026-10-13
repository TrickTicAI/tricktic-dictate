"""Deterministic lockfile inventory. Scope is evidence, never a reachability waiver."""
import hashlib
import json
import re
import tomllib
from pathlib import Path
from urllib.parse import quote

import yaml


def digest(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def purl(ecosystem, name, version):
    kind = {'crates.io': 'cargo', 'npm': 'npm', 'Maven': 'maven', 'native': 'generic'}[ecosystem]
    name = name.replace(':', '/') if ecosystem == 'Maven' else name
    return f'pkg:{kind}/{quote(name, safe="/")}@{quote(version, safe="")}'


def component(ecosystem, name, version, scopes, evidence, **extra):
    return dict(ecosystem=ecosystem, name=name, version=version,
                purl=purl(ecosystem, name, version), scopes=sorted(set(scopes)),
                evidence=[evidence], **extra)


def cargo(root):
    lock = tomllib.loads((root / 'Cargo.lock').read_text())
    packages = lock['package']
    scopes = {(p['name'], p['version']): set() for p in packages}
    by_name = {}
    for package in packages:
        by_name.setdefault(package['name'], []).append(package)
        source = package.get('source', '')
        if source and source != 'registry+https://github.com/rust-lang/crates.io-index':
            raise ValueError('non-crates.io Cargo sources require a reviewed inventory adapter')

    def walk(name, scope, seen):
        if name in seen:
            return
        seen.add(name)
        # Lockfiles do not retain dependency kind/target per edge. Include all
        # matching versions and transitive edges conservatively, never subtract.
        for package in by_name.get(name, []):
            scopes[(package['name'], package['version'])].add(scope)
            for dependency in package.get('dependencies', []):
                walk(dependency.split()[0], scope, seen)

    workspace = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']
    for member in workspace['members']:
        manifest = tomllib.loads((root / member / 'Cargo.toml').read_text())
        for table, target_specific in [(manifest, False)] + [(v, True) for v in manifest.get('target', {}).values()]:
            for group, scope in [('dependencies', 'platform' if target_specific else 'production'),
                                 ('build-dependencies', 'build'), ('dev-dependencies', 'test')]:
                for name, dependency in table.get(group, {}).items():
                    if isinstance(dependency, dict) and dependency.get('workspace'):
                        dependency = workspace.get('dependencies', {}).get(name, dependency)
                    actual = dependency.get('package', name) if isinstance(dependency, dict) else name
                    walk(actual, scope, set())
    return [component('crates.io', p['name'], p['version'], scopes[(p['name'], p['version'])] or ['platform'],
                      'Cargo.lock', checksum=p.get('checksum'),
                      target='all locked targets; root scopes propagated conservatively; transitive kinds unreviewed')
            for p in packages if p.get('source')]


def pnpm(root):
    lock = yaml.safe_load((root / 'pnpm-lock.yaml').read_text())
    if str(lock['lockfileVersion']) != '9.0':
        raise ValueError('unsupported pnpm lock version')
    packages, snapshots = lock['packages'], lock['snapshots']
    scopes = {key: set() for key in packages}

    def walk(name, version, scope, seen):
        if version.startswith(('link:', 'workspace:')):
            return
        key = name + '@' + version
        base = key.split('(')[0]
        if key in seen:
            return
        seen.add(key)
        if base not in packages or key not in snapshots:
            raise ValueError('pnpm dependency missing from lock graph')
        scopes[base].add(scope)
        for group in ('dependencies', 'optionalDependencies'):
            for child, value in snapshots[key].get(group, {}).items():
                walk(child, value, scope, seen)

    for importer, groups in lock['importers'].items():
        for group in ('dependencies', 'optionalDependencies', 'devDependencies'):
            for name, value in groups.get(group, {}).items():
                scope = 'production' if group != 'devDependencies' and importer == 'apps/web' else 'build'
                if 'playwright' in name or name in ('vitest', 'jest'):
                    scope = 'test'
                walk(name, value['version'], scope, set())
    result = []
    for key in sorted(packages):
        name, version = key.rsplit('@', 1)
        result.append(component('npm', name, version, scopes[key] or ['platform'], 'pnpm-lock.yaml',
                                integrity=packages[key].get('resolution', {}).get('integrity')))
    return result


def maven(root):
    result = []
    for path in sorted(root.glob('apps/android/*/gradle.lockfile')):
        for line in path.read_text().splitlines():
            if not line or line.startswith('#') or line.startswith('empty='):
                continue
            coords, configs = line.split('=', 1)
            group, name, version = coords.split(':')
            scopes = set()
            for config in configs.split(','):
                if config == 'releaseRuntimeClasspath' and path.parent.name == 'app':
                    scopes.add('production')
                elif 'test' in config.lower() or path.parent.name == 'editor':
                    scopes.add('test')
                else:
                    scopes.add('build')
            result.append(component('Maven', group + ':' + name, version, scopes,
                                    path.relative_to(root).as_posix(), configurations=configs.split(',')))
    if not result:
        raise ValueError('no Maven lock entries')
    return result


def native(root, platform):
    result = []
    platforms = ['windows', 'macos'] if platform == 'all' else [platform]
    for target in platforms:
        path = root / f'scripts/desktop/{target}-runtime.lock.json'
        if not path.exists():
            continue
        lock = json.loads(path.read_text())
        for item in lock['files']:
            if item['role'] != 'runtime':
                continue
            result.append(component('native', item['path'], 'sha256-' + item['sha256'],
                                    ['production'], path.relative_to(root).as_posix(),
                                    checksum=item['sha256'], bytes=item['bytes'], platform=target,
                                    provenance='hash-only; upstream component identity requires artifact manifest'))
    if platform in ('all', 'android'):
        result.append(component('native', 'android-jni-runtime', 'unresolved', ['production'],
                                'apps/android/app/src/main/cpp/CMakeLists.txt', platform='android',
                                provenance='externally supplied native source; inspect built APK payload'))
    return result


def inventory(root, ecosystem='all', platform='all'):
    entries = []
    if ecosystem in ('cargo', 'all') and platform != 'android':
        entries += cargo(root)
    if ecosystem in ('pnpm', 'all') and platform != 'android':
        entries += pnpm(root)
    if ecosystem in ('maven', 'all') and platform in ('android', 'all'):
        entries += maven(root)
    if ecosystem in ('native', 'all'):
        entries += native(root, platform)
    merged = {}
    for entry in entries:
        key = (entry['purl'], entry.get('platform', ''))
        if key in merged:
            for field in ('scopes', 'evidence', 'configurations'):
                if field in entry:
                    merged[key][field] = sorted(set(merged[key].get(field, []) + entry[field]))
        else:
            merged[key] = entry
    return sorted(merged.values(), key=lambda c: (c['purl'], c.get('platform', '')))


def sbom(entries, platform, commit):
    components = []
    for item in entries:
        ref = item['purl'] + ('?platform=' + item['platform'] if item.get('platform') else '')
        c = {'type': 'file' if item['ecosystem'] == 'native' else 'library',
             'bom-ref': ref, 'name': item['name'], 'version': item['version'], 'purl': item['purl'],
             'properties': [{'name': 'writer:scopes', 'value': ','.join(item['scopes'])},
                            {'name': 'writer:evidence', 'value': ','.join(item['evidence'])}]}
        if item.get('checksum'):
            c['hashes'] = [{'alg': 'SHA-256', 'content': item['checksum']}]
        components.append(c)
    return {'bomFormat': 'CycloneDX', 'specVersion': '1.6', 'version': 1,
            'metadata': {'component': {'type': 'application', 'name': 'TrickTic Dictate', 'version': commit},
                         'properties': [{'name': 'writer:platform', 'value': platform},
                                        {'name': 'writer:coverage', 'value': 'conservative lock inventory; not proof of code reachability'}]},
            'components': components}
