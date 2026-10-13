#!/usr/bin/env python3
"""Test packaged FFmpeg with synthetic media and malformed inputs, without private audio.
Generate with --generate /usr/bin/ffmpeg, copy fixtures to each native host, then test.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import time
import wave

FORMATS = {'wav': ('wav', 'pcm_s16le'), 'webm': ('matroska', 'libopus'),
           'ogg': ('ogg', 'libvorbis'), 'm4a': ('mov', 'aac'), 'mp3': ('mp3', 'libmp3lame')}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--fixtures', type=Path, required=True)
    p.add_argument('--generate', type=Path)
    p.add_argument('--ffmpeg', type=Path)
    p.add_argument('--receipt', type=Path)
    args = p.parse_args()
    if args.generate:
        args.fixtures.mkdir(parents=True, exist_ok=False)
        with wave.open(str(args.fixtures/'tone.wav'), 'wb') as wav:
            wav.setparams((1, 2, 48000, 0, 'NONE', 'not compressed'))
            wav.writeframes(b''.join(struct.pack('<h', round(8000*math.sin(2*math.pi*440*i/48000)))
                                     for i in range(48000)))
        for suffix, (_, codec) in FORMATS.items():
            if suffix != 'wav':
                subprocess.run([str(args.generate), '-nostdin', '-v', 'error', '-i',
                                str(args.fixtures/'tone.wav'), '-c:a', codec,
                                str(args.fixtures/('tone.'+suffix))], check=True)
        for suffix in FORMATS:
            (args.fixtures/('broken.'+suffix)).write_bytes((args.fixtures/('tone.'+suffix)).read_bytes()[:4])
        (args.fixtures/'playlist.txt').write_text('#EXTM3U\nhttp://127.0.0.1:9/private\n')
        return
    if not args.ffmpeg or not args.receipt:
        p.error('--ffmpeg and --receipt are required for validation')
    before = {f.name: digest(f) for f in args.fixtures.iterdir() if f.is_file()}
    env = os.environ.copy()
    env['ASAN_OPTIONS'] = 'detect_leaks=1:halt_on_error=1'
    env['UBSAN_OPTIONS'] = 'halt_on_error=1:print_stacktrace=1'
    results = []
    def run(source, fmt, success):
        input_path = str(source)
        if os.name != 'nt' and args.ffmpeg.suffix == '.exe':
            input_path = subprocess.check_output(['wslpath', '-w', str(source.resolve())], text=True).strip()
        command = [str(args.ffmpeg), '-nostdin', '-v', 'error', '-protocol_whitelist',
                   'file,pipe', '-f', fmt, '-i', input_path, '-vn', '-ar', '16000',
                   '-ac', '1', '-c:a', 'pcm_s16le', '-f', 's16le', 'pipe:1']
        start = time.monotonic()
        result = subprocess.run(command, capture_output=True, timeout=20, env=env)
        duration = time.monotonic() - start
        stderr = result.stderr.decode(errors='replace')
        assert not any(s in stderr for s in ['AddressSanitizer', 'LeakSanitizer', 'runtime error:']), stderr
        assert (result.returncode == 0) == success, (source.name, result.returncode, stderr)
        assert result.returncode >= 0, (source.name, 'signal', result.returncode)
        if success:
            assert 30000 <= len(result.stdout) <= 35000, (source.name, len(result.stdout))
            samples = struct.unpack('<'+'h'*(len(result.stdout)//2), result.stdout)
            rms = math.sqrt(sum(x*x for x in samples)/len(samples))
            assert 4000 < rms < 7000, (source.name, rms)
        results.append({'fixture': source.name, 'format': fmt, 'exit': result.returncode,
                        'seconds': round(duration, 4), 'pcm_bytes': len(result.stdout)})
    for suffix, (fmt, _) in FORMATS.items():
        run(args.fixtures/('tone.'+suffix), fmt, True)
        run(args.fixtures/('broken.'+suffix), fmt, False)
        run(args.fixtures/'playlist.txt', fmt, False)
    assert before == {f.name: digest(f) for f in args.fixtures.iterdir() if f.is_file()}, 'Input changed'
    protocols = subprocess.check_output([str(args.ffmpeg), '-hide_banner', '-protocols'], text=True)
    assert set(protocols.split()) == {'Supported', 'file', 'pipe', 'protocols:', 'Input:', 'Output:'}, protocols
    args.receipt.write_text(json.dumps({'binary_sha256': digest(args.ffmpeg), 'fixtures': before,
               'cases': results, 'inputs_unchanged': True, 'network_protocols': False}, indent=2)+'\n')
    print(f'{len(results)} media cases passed; inputs unchanged; file/pipe protocols only')


if __name__ == '__main__':
    main()
