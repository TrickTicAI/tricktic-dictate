#!/usr/bin/env python3
"""Build the public duration suite (MW-ANDROID-01) outside Git, with human references.

English: consecutive utterances of one LibriSpeech test-clean speaker's chapters (CC BY 4.0,
one speaker, read speech, chapter then utterance order), joined in corpus order with 300 ms silence into ~30 s, ~5 min
and a long clip (that speaker's complete audio, at least 7 min; no test-clean speaker has 12). Hebrew: distinct FLEURS he_il test sentences (CC BY 4.0, several
speakers) joined into ~30 s and ~5 min clips. Both are constructed concatenations of
natural recordings; neither is accented, mixed-language or the maintainer's speech. Only
lossless FLAC decode (no resampling) and the FLEURS float→PCM16 conversion apply.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tarfile
import wave

from device import digest, private_path

GAP = b'\0\0' * 4800  # 300 ms of silence at 16 kHz PCM16
TARGETS = {'30s': 30, '5min': 300, 'long': 10 ** 6}  # long = all of the speaker's audio
LONG_MINIMUM_S = 420


def flac_pcm(data):
    # LibriSpeech is already 16 kHz mono; ffmpeg only decodes FLAC losslessly.
    return subprocess.run(['ffmpeg', '-v', 'error', '-f', 'flac', '-i', 'pipe:0', '-f', 's16le', '-ac', '1', '-ar', '16000', 'pipe:1'],
                          input=data, capture_output=True, check=True).stdout


def write(path, pcm):
    with wave.open(str(path), 'wb') as out:
        out.setparams((1, 2, 16000, 0, 'NONE', 'not compressed')); out.writeframes(pcm)


def english(archive, out):
    chapters = {}
    with tarfile.open(archive) as tar:
        members = {m.name: m for m in tar if m.isfile()}
        for name, member in members.items():
            if name.endswith('.trans.txt'):
                chapters[name] = tar.extractfile(member).read().decode().splitlines()
        # The speaker with the most audio, so the 12-minute clip needs no repetition.
        speakers = {}
        for name, lines in chapters.items():
            folder = name.rsplit('/', 1)[0]
            for line in sorted(lines):
                speakers.setdefault(folder.rsplit('/', 2)[-2], []).append((folder + '/' + line.split(' ', 1)[0] + '.flac', line.split(' ', 1)[1]))
        speaker = max(speakers, key=lambda k: sum(members[f].size for f, _ in speakers[k]))
        utterances = sorted(speakers[speaker])
        audio, text = [], []
        for flac, words in utterances:
            audio.append(flac_pcm(tar.extractfile(members[flac]).read())); text.append(words)
        chapter = f'speaker {speaker}'
    clips = []
    for label, seconds in TARGETS.items():
        pcm, words, used = b'', [], 0
        for chunk, sentence in zip(audio, text):
            if len(pcm) >= seconds * 32000: break
            pcm += (GAP if pcm else b'') + chunk; words.append(sentence); used += 1
        if len(pcm) < min(seconds * 0.9, LONG_MINIMUM_S) * 32000: raise ValueError(f'Speaker audio too short for {label}')
        path = out / f'libri-en-{label}.wav'; write(path, pcm)
        clips.append({'id': f'libri-en-{label}', 'wav': str(path), 'sha256': digest(path), 'mode': 'en', 'consented': True,
                      'reference_kind': 'public_reference', 'reference': ' '.join(words), 'audio_ms': len(pcm) // 32,
                      'provenance': f'Public LibriSpeech test-clean (CC BY 4.0), {chapter}, first {used} consecutive utterances; one native speaker, read speech; not maintainer speech',
                      'construction': 'consecutive utterances joined with 300 ms silence; lossless FLAC decode, no resampling'})
    return clips


def hebrew(fleurs, out):
    import importlib
    convert = importlib.import_module('fetch-public-he').convert_float_wav
    rows = [line.split('\t') for line in (fleurs / 'test.tsv').read_text().splitlines()]
    seen, chosen = set(), []
    for row in rows:  # Distinct sentences only: FLEURS repeats each sentence per speaker.
        if row[0] not in seen: seen.add(row[0]); chosen.append(row)
    wanted = {r[1] for r in chosen}
    sources = {}
    with tarfile.open(fleurs / 'test.tar.gz') as tar:
        for member in tar:
            name = Path(member.name).name
            if member.isfile() and name in wanted: sources[name] = tar.extractfile(member).read()
    clips = []
    for label in ('30s', '5min'):
        seconds = TARGETS[label]; pcm, words, ids = b'', [], []
        for row in chosen:
            if len(pcm) >= seconds * 32000: break
            source, target = out / 'fleurs-source.wav', out / 'fleurs-row.wav'
            source.write_bytes(sources[row[1]]); target.unlink(missing_ok=True); convert(source, target)
            with wave.open(str(target), 'rb') as clip: chunk = clip.readframes(clip.getnframes())
            pcm += (GAP if pcm else b'') + chunk; words.append(row[2]); ids.append(row[0])
        for temporary in (out / 'fleurs-source.wav', out / 'fleurs-row.wav'): temporary.unlink(missing_ok=True)
        path = out / f'fleurs-he-{label}.wav'; write(path, pcm)
        clips.append({'id': f'fleurs-he-{label}', 'wav': str(path), 'sha256': digest(path), 'mode': 'he', 'consented': True,
                      'reference_kind': 'public_reference', 'reference': ' '.join(words), 'audio_ms': len(pcm) // 32,
                      'provenance': f'Public google/fleurs he_il test (CC BY 4.0), {len(ids)} distinct sentences, several speakers; not maintainer speech',
                      'construction': 'distinct sentences joined in table order with 300 ms silence; float32→PCM16 per fetch-public-he.py'})
    return clips


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--librispeech', type=Path, required=True, help='test-clean.tar.gz from openslr.org/12')
    p.add_argument('--librispeech-sha256')
    p.add_argument('--fleurs', type=Path, required=True, help='Directory written by fetch-public-he.py')
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    archive = private_path(a.librispeech); out = private_path(a.output); out.mkdir(parents=True, exist_ok=False)
    archive_sha = digest(archive)
    if a.librispeech_sha256 and archive_sha != a.librispeech_sha256: raise ValueError('LibriSpeech archive hash mismatch')
    clips = english(archive, out) + hebrew(private_path(a.fleurs), out)
    (out / 'suite.json').write_text(json.dumps({'librispeech_sha256': archive_sha, 'clips': clips}, ensure_ascii=False, indent=2) + '\n')
    for clip in clips: print(clip['id'], round(clip['audio_ms'] / 1000, 1), 's')


if __name__ == '__main__':
    os.umask(0o077)
    main()
