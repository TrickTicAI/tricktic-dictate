#!/usr/bin/env python3
"""Prepare three pinned public FLEURS Hebrew test clips outside Git (CC BY 4.0)."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import tarfile
import urllib.request
import wave

from device import digest, private_path

REVISION = '70bb2e84b976b7e960aa89f1c648e09c59f894dd'
BASE = f'https://huggingface.co/datasets/google/fleurs/resolve/{REVISION}/data/he_il/'
ARCHIVE_SHA = 'f09f2335973d7ff5469c0e88fc37b528e66ec2ff2af5346438daacb3fbd84a53'


def convert_float_wav(source, target):
    # FLEURS original audio is IEEE float WAV; make the conversion explicit.
    data = source.read_bytes()
    if data[:4] != b'RIFF' or data[8:12] != b'WAVE':
        raise ValueError('Not RIFF/WAVE')
    offset, fmt, pcm = 12, None, None
    while offset + 8 <= len(data):
        kind, size = struct.unpack_from('<4sI', data, offset)
        payload = data[offset + 8:offset + 8 + size]
        if len(payload) != size:
            raise ValueError('Truncated RIFF chunk')
        if kind == b'fmt ': fmt = payload
        if kind == b'data': pcm = payload
        offset += 8 + size + size % 2
    if fmt is None or pcm is None:
        raise ValueError('Missing WAV chunks')
    codec, channels, rate, _, _, bits = struct.unpack_from('<HHIIHH', fmt)
    if (codec, channels, rate, bits) != (3, 1, 16000, 32) or len(pcm) % 4:
        raise ValueError('Expected mono 16kHz IEEE-float32 FLEURS source')
    frames = bytearray()
    for (value,) in struct.iter_unpack('<f', pcm):
        if not math.isfinite(value): raise ValueError('Nonfinite sample')
        frames.extend(struct.pack('<h', min(32767, max(-32768, round(value * 32768)))))
    with wave.open(str(target), 'wb') as out:
        out.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
        out.writeframes(frames)
    return len(frames) // 2


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', type=Path, required=True, help='New private directory; downloads 387 MB archive')
    a = p.parse_args()
    out = private_path(a.output)
    out.mkdir(parents=True, exist_ok=False)
    archive, table = out / 'test.tar.gz', out / 'test.tsv'
    urllib.request.urlretrieve(BASE + 'audio/test.tar.gz', archive)
    if digest(archive) != ARCHIVE_SHA:
        raise ValueError('Archive hash mismatch')
    urllib.request.urlretrieve(BASE + 'test.tsv', table)
    # Pin the TSV as a Git blob as well as the dataset revision.
    content = table.read_bytes()
    blob = hashlib.sha1(f'blob {len(content)}\0'.encode() + content).hexdigest()
    if blob != '966119755c9c078ae2e8c8de06d0069baec84f15':
        raise ValueError('Reference table hash mismatch')
    rows = [line.split('\t') for line in content.decode().splitlines()[:3]]
    wanted = {r[1] for r in rows}
    with tarfile.open(archive) as tar:
        for member in tar:
            name = Path(member.name).name
            if name in wanted and member.isfile():
                # Copy only named file bytes, never extract archive paths/links.
                (out / ('source-' + name)).write_bytes(tar.extractfile(member).read())
    clips = []
    for index, row in enumerate(rows):
        source, wav = out / ('source-' + row[1]), out / f'fleurs-he-{index}.wav'
        samples = convert_float_wav(source, wav)
        if samples != int(row[5]):
            raise ValueError('FLEURS sample count differs from reference table')
        clips.append({'id': f'fleurs-he-{index}', 'wav': str(wav), 'sha256': digest(wav), 'mode': 'he',
                      'consented': True, 'reference_kind': 'public_reference', 'reference': row[2], 'source_sha256': digest(source),
                      'provenance': f'Public google/fleurs CC BY 4.0, {REVISION}, he_il test row {index}, id {row[0]}, file {row[1]}; not maintainer speech',
                      'conversion': 'IEEE-float32 to PCM16: round(sample*32768), saturate [-32768,32767]; no resampling'})
    (out / 'suite.json').write_text(json.dumps({'clips': clips}, ensure_ascii=False, indent=2) + '\n')
    print('Private FLEURS suite: ' + str(out / 'suite.json'))


if __name__ == '__main__':
    os.umask(0o077)
    main()
