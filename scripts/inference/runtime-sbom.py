#!/usr/bin/env python3
"""Generate a CycloneDX source/patch inventory from reviewed Git objects (no latest refs).
--sources contains whisper-upstream, llama-upstream and nemo-upstream clones.
The source inventory is not a claim of reproducible third-party release binaries.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
MAC_TEXT = json.loads((ROOT / 'scripts/inference/macos-sources.json').read_text())
PINS = [
    ('whisper-desktop', 'whisper-upstream', 'ggml-org/whisper.cpp', '371b5a7561823ab2bb32142d2751e35e7534727b'),
    ('whisper-android', 'whisper-upstream', 'ggml-org/whisper.cpp', '927cfce34f31707e17f2bff35c349632fb9e2c3a'),
    ('llama-text', 'llama-upstream', 'ggml-org/llama.cpp', MAC_TEXT['llama']['revision']),
    ('llama-text-windows', 'llama-upstream', 'ggml-org/llama.cpp', 'dbe4c3ed42343f0a7ba0fd7e808ffeaca404d29f'),
    ('nemo', 'nemo-upstream', 'NVIDIA/NeMo-Speech.cpp', '4f9676226f667d14608487df744f375db87127f8'),
]
PATCHES = ['scripts/inference/stage-whisper.py', 'scripts/inference/encoder-checkpoints.py',
           'scripts/inference/harden-whisper-model.py', 'runtime/whisper/writer-scheduler.h',
           'runtime/whisper/writer-auth.h', 'runtime/whisper/writer-http-limits.h',
           'runtime/whisper/writer.manifest', 'runtime/whisper/httplib-source.json',
           'scripts/inference/stage-nemo.py', 'scripts/desktop/patch-nemo-utf8.ps1']
# Private-worker authentication (MW-SEC-2) patches both desktop Whisper and the
# source-built macOS NeMo workers; Windows NeMo stays the upstream release binary.
NEMO_PATCHES = ['runtime/whisper/writer-auth.h', 'scripts/inference/stage-nemo.py',
                'scripts/desktop/patch-nemo-utf8.ps1']


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--sources', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    components, dependencies = [], []
    def add(ref, name, version, url=None, data=None, properties=None, kind='library'):
        entry = {'type': kind, 'bom-ref': ref, 'name': name, 'version': version}
        if url: entry['externalReferences'] = [{'type': 'vcs', 'url': url}]
        if data is not None: entry['hashes'] = [{'alg': 'SHA-256', 'content': hashlib.sha256(data).hexdigest()}]
        if properties: entry['properties'] = [{'name': k, 'value': v} for k, v in properties.items()]
        components.append(entry)
    for ref, folder, repository, revision in PINS:
        repo = args.sources/folder
        def git(*argv):
            return subprocess.check_output(['git', '-C', str(repo), *argv])
        assert git('rev-parse', revision+'^{commit}').decode().strip() == revision
        tree = git('rev-parse', revision+'^{tree}').decode().strip()
        add(ref, repository, revision, 'https://github.com/'+repository+'/tree/'+revision,
            properties={'writer:git-tree': tree, 'writer:binary-attribution':
                        'source-build' if ref.startswith('whisper') else
                        'macOS source-build with Writer private-worker patch; Windows upstream-release'
                        if ref == 'nemo' else 'source-build with pinned miniaudio replacement; native build receipts required'})
        children = []
        for line in git('ls-tree', '-r', revision).decode().splitlines():
            mode, kind, oid, name = line.split(maxsplit=3)
            selected = mode == '160000' or name.startswith('ggml-patches/') and name.endswith('.patch')
            selected |= name in ['examples/server/httplib.h', 'examples/stb_vorbis.c',
                                 'vendor/cpp-httplib/httplib.h', 'vendor/cpp-httplib/httplib.cpp', 'vendor/miniaudio/miniaudio.h',
                                 'vcpkg.json', 'CMakePresets.json', 'THIRD_PARTY_NOTICES.md']
            if not selected: continue
            child = ref+':'+name; children.append(child)
            data = git('show', revision+':'+name) if kind == 'blob' else None
            add(child, repository+'/'+name, oid, data=data, kind='file' if kind == 'blob' else 'library')
        ggml = git('ls-tree', revision, 'ggml').decode().split()
        if ggml and ggml[1] == 'tree':
            child = ref+':ggml'; children.append(child)
            add(child, 'ggml', ggml[2], properties={'writer:version-scheme': 'vendored Git tree in parent revision'})
        dependencies.append({'ref': ref, 'dependsOn': children})
    for path in PATCHES:
        data = (ROOT/path).read_bytes()
        add(path, path, hashlib.sha256(data).hexdigest(), data=data, kind='file')
    # MW-SEC-26: desktop Whisper replaces its vendored cpp-httplib with this pin.
    httplib = json.loads((ROOT/'runtime/whisper/httplib-source.json').read_text())
    add('whisper-desktop:cpp-httplib', httplib['name'], httplib['version'],
        'https://github.com/'+httplib['name']+'/tree/'+httplib['revision'], properties={
            'writer:git-revision': httplib['revision'], 'writer:header-sha256': httplib['sha256'],
            'writer:replaces': 'whisper-desktop:examples/server/httplib.h'})
    # MW-SEC-26: the Windows text runtime is built from source with this miniaudio.
    text = json.loads((ROOT/'scripts/inference/windows-text-source.json').read_text())
    add('llama-text-windows:miniaudio', 'mackron/miniaudio', text['miniaudio']['revision'],
        'https://github.com/mackron/miniaudio/tree/'+text['miniaudio']['revision'], properties={
            'writer:header-sha256': text['miniaudio']['sha256'],
            'writer:replaces': 'llama-text-windows:vendor/miniaudio/miniaudio.h'})
    for path in ['scripts/inference/windows-text-source.json', 'scripts/inference/build-windows-text.py']:
        data = (ROOT/path).read_bytes()
        add(path, path, hashlib.sha256(data).hexdigest(), data=data, kind='file')
    add('llama-text:miniaudio', 'mackron/miniaudio', MAC_TEXT['miniaudio']['revision'],
        'https://github.com/mackron/miniaudio/tree/' + MAC_TEXT['miniaudio']['revision'], properties={
            'writer:header-sha256': MAC_TEXT['miniaudio']['sha256'],
            'writer:license-sha256': MAC_TEXT['miniaudio']['license_sha256'],
            'writer:replaces': 'llama-text:vendor/miniaudio/miniaudio.h'})
    mac_text_inputs = ['scripts/inference/macos-sources.json', 'scripts/inference/stage-macos-text.py',
                       'scripts/inference/build-macos-text.py', 'scripts/desktop/replace-macos-text.py']
    for path in mac_text_inputs:
        data = (ROOT / path).read_bytes()
        add(path, path, hashlib.sha256(data).hexdigest(), data=data, kind='file')
    ffmpeg = json.loads((ROOT/'scripts/inference/ffmpeg-source.json').read_text())
    add('ffmpeg', 'FFmpeg', ffmpeg['version'], properties={
        'writer:source-archive-sha256': ffmpeg['sha256'], 'writer:source-url': ffmpeg['source_url'],
        'writer:verified-release-signer': ffmpeg['signer_fingerprint']})
    add('nemo:vcpkg', 'vcpkg baseline', '9e593bb18ea69cc5095e012465dcd675a822ed0d',
        'https://github.com/microsoft/vcpkg/tree/9e593bb18ea69cc5095e012465dcd675a822ed0d')
    # Resolved from that exact baseline's versions/baseline.json, not current ports.
    vcpkg_dependencies = []
    for name, version in [('sentencepiece', '0.2.1#0'), ('protobuf', '6.33.4#2'),
                          ('abseil', '20260107.1#3'), ('utf8-range', '6.33.4#0')]:
        ref = 'nemo:vcpkg:'+name
        vcpkg_dependencies.append(ref)
        add(ref, name, version, properties={
            'writer:evidence': 'NeMo declared vcpkg baseline 9e593bb18ea69cc5095e012465dcd675a822ed0d; source resolution, not binary attestation'})
    dependencies.append({'ref': 'nemo:vcpkg', 'dependsOn': vcpkg_dependencies})
    for name, version, repo in [
        ('sentencepiece', '17d7580d6407802f85855d2cc9190634e2c95624', 'google/sentencepiece'),
        ('miniaudio', '9634bedb5b5a2ca38c1ee7108a9358a4e233f14d', 'mackron/miniaudio'),
        ('parakeet.cpp', '1675ee5b612b629645e3bb4b42f671da27a76d5b', 'jason-ni/parakeet.cpp')]:
        add('nemo:'+name, name, version, 'https://github.com/'+repo+'/tree/'+version,
            properties={'writer:evidence': 'NeMo v0.1.0 THIRD_PARTY_NOTICES.md; source attribution'})
    for entry in dependencies:
        if entry['ref'].startswith('whisper-'):
            entry['dependsOn'] += ([p for p in PATCHES if p not in NEMO_PATCHES[1:]]
                                   + ['whisper-desktop:cpp-httplib'] if entry['ref'] == 'whisper-desktop'
                                   else ['scripts/inference/harden-whisper-model.py'])
        elif entry['ref'] == 'llama-text-windows':
            entry['dependsOn'] += ['llama-text-windows:miniaudio', 'scripts/inference/windows-text-source.json',
                                   'scripts/inference/build-windows-text.py']
        elif entry['ref'] == 'llama-text':
            entry['dependsOn'] += ['llama-text:miniaudio', *mac_text_inputs]
        elif entry['ref'] == 'nemo':
            entry['dependsOn'] += ['nemo:vcpkg', 'nemo:sentencepiece', 'nemo:miniaudio',
                                   'nemo:parakeet.cpp', *NEMO_PATCHES]
    doc = {'bomFormat': 'CycloneDX', 'specVersion': '1.6', 'version': 1,
           'metadata': {'timestamp': '2026-09-20T00:00:00Z', 'properties': [
               {'name': 'writer:scope', 'value': 'Native source and patch inventory. Exact packaged bytes are in platform locks; retained upstream build toolchains are not attested.'}]},
           'components': components, 'dependencies': dependencies}
    args.output.write_text(json.dumps(doc, indent=2)+'\n')


if __name__ == '__main__':
    main()
