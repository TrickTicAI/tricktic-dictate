#!/usr/bin/env python3
"""Refresh the curated Whisper artifact manifest from public Hugging Face metadata.
This development tool downloads metadata only. Review the diff and qualify new
artifacts before publication; the app never runs code from model repositories.
"""
import json
from pathlib import Path
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parents[2]
REPOS = {
    'ggerganov/whisper.cpp': '5359861c739e955e79d9a303bcbc70fb988958b1',
    'ivrit-ai/whisper-large-v3-ggml': '9ead614052ce13dfe5f8d0f6cd3e36787a9cf60c',
    'ivrit-ai/whisper-large-v3-turbo-ggml': '2130c78e4a9cb4914cc4df91a1c3031407789705',
    'distil-whisper/distil-large-v3.5-ggml': '960ecb5c2ecfba3ebb9ebe485c1032ec266cf436',
}
proof_path = ROOT / 'apps/api/src/desktop_settings/model-smoke-tests.json'
proofs = json.loads(proof_path.read_text()) if proof_path.exists() else {}
rows = []
for repo, revision in REPOS.items():
    with urlopen(f'https://huggingface.co/api/models/{repo}/revision/{revision}?blobs=true', timeout=60) as response:
        data = json.load(response)
    assert data['sha'] == revision
    for artifact in data['siblings']:
        filename = artifact['rfilename']
        if not filename.startswith('ggml-') or not filename.endswith('.bin'):
            continue
        precision = next((p for p in ['q5_0', 'q5_1', 'q8_0'] if p in filename), 'f16')
        if repo.startswith('ggerganov/'):
            name = filename[5:-4]
            identity = 'whisper-' + name.replace('.', '-') .replace('_', '-')
            family = name.split('-q')[0]
            languages = ['en'] if '.en' in name else ['en', 'he', 'mixed']
            title = 'Whisper ' + family
            notes = 'English only.' if languages == ['en'] else 'Multilingual. Mixed speech quality must be evaluated separately.'
            target = filename
        elif repo.startswith('ivrit-ai/'):
            turbo = 'turbo' in repo
            identity = 'ivrit-turbo' if turbo else 'ivrit-large-v3'
            family = identity
            languages = ['he']
            title = 'ivrit.ai Whisper ' + ('large-v3-turbo' if turbo else 'large-v3')
            notes = 'Hebrew fine-tune. English and mixed speech are not enabled.'
            target = identity + '.bin'
        else:
            identity = family = 'distil-large-v3-5'
            languages = ['en']
            title = 'Distil-Whisper large-v3.5'
            notes = 'English-only distilled Whisper model.'
            target = identity + '.bin'
        rows.append(dict(id=identity, family=family, display_name=title, precision=precision,
                         file_name=target, source_name=repo, revision=revision,
                         source_url=f'https://huggingface.co/{repo}/resolve/{revision}/{filename}',
                         bytes=artifact['size'], sha256=artifact['lfs']['sha256'],
                         license=data['cardData']['license'], notes=notes,
                         input_languages=languages, startup_language='he' if languages == ['he'] else 'en' if languages == ['en'] else 'auto',
                         runtime='whisper_cpp', backends=['cpu', 'vulkan'], qualification='windows_amd_smoke_tested' if proofs.get(identity, {}).get('model_sha256') == artifact['lfs']['sha256'] else 'not_evaluated'))
assert len({row['id'] for row in rows}) == len(rows)
path = ROOT / 'apps/api/src/desktop_settings/model-catalog.json'
rows += [m for m in json.loads(path.read_text()) if m.get('runtime') != 'whisper_cpp']
path.write_text(json.dumps(rows, indent=2, ensure_ascii=False) + '\n')
print(f'{len(rows)} pinned artifacts written to {path.relative_to(ROOT)}')
