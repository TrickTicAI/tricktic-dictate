#!/usr/bin/env python3
"""Stop-to-text trial for live and chunked cloud transcription (MW-ANDROID-01).

Replays public suite clips at real-time pace, as if dictated, and measures the time from
the end of speech (Stop) to complete text:

  chunked   cut at the quietest 100 ms between 20 and 30 s after the previous cut; each
            chunk goes to the batch Gemini request (same as cloud-gemini.py) as soon as
            it has been "spoken"
  live      100 ms PCM frames to the Gemini Live transcription model over WebSocket;
            final text after audioStreamEnd
  live-cut  live, with the socket closed at --cut-at of the clip; the whole recording
            then falls back to chunked: past chunks are sent at once, in parallel, and
            later ones as they are spoken; live text is discarded, so nothing is
            duplicated or lost at the seam

The key comes from GEMINI_API_KEY and is never printed or written. Requires the
`websockets` package (PYTHONPATH). Transcripts stay in the private output directory.
Host-network timing, not phone-network timing.
"""
import argparse
import asyncio
import audioop
import base64
import importlib
import json
import os
from pathlib import Path
import time
import wave

import websockets

from device import private_path, tokens, wer

cloud = importlib.import_module('cloud-gemini')
LIVE = 'wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent'
LANGUAGES = {'en': ['en-US'], 'he': ['he-IL'], 'mixed': []}
RATE, FRAME = 16000, 3200  # bytes per 100 ms of PCM16 mono


def cuts(pcm, low=20, high=30):
    """Chunk boundaries in bytes, at the quietest 100 ms frame in each search window."""
    frames = [audioop.rms(pcm[i:i + FRAME], 2) for i in range(0, len(pcm), FRAME)]
    bounds, start = [0], 0
    while (len(frames) - start) * 0.1 > high:
        window = range(start + low * 10, min(start + high * 10, len(frames)))
        start = min(window, key=lambda i: frames[i]) + 1
        bounds.append(start * FRAME)
    return bounds + [len(pcm)]


def write_wav(path, pcm):
    with wave.open(str(path), 'wb') as out:
        out.setparams((1, 2, RATE, 0, 'NONE', 'not compressed')); out.writeframes(pcm)


async def batch_chunks(key, model, pcm, mode, bounds, t0, scratch, thinking=None):
    """Send each chunk when spoken (or at once if already spoken). Returns (text, done_time, rows).

    A chunk the provider rejects is retried once merged with its predecessor (or successor
    for the first), since the same audio inside a larger window is accepted; the merged text
    then replaces both chunks."""
    async def request(name, start, end):
        path = scratch / f'{name}.wav'; write_wav(path, pcm[start:end])
        text, timing, _ = await asyncio.to_thread(cloud.transcribe, key, model, path, mode, thinking)
        return text, timing['total_ms']
    async def one(i):
        end_s = bounds[i + 1] / (2 * RATE)
        delay = t0 + end_s - time.monotonic()
        if delay > 0: await asyncio.sleep(delay)
        sent = time.monotonic(); row = {'chunk': i, 'audio_s': round((bounds[i + 1] - bounds[i]) / (2 * RATE), 1), 'sent_after_spoken_ms': round((sent - t0 - end_s) * 1000)}
        try:
            row['text'], row['request_ms'] = await request(f'chunk-{i}', bounds[i], bounds[i + 1])
        except RuntimeError:
            first, last = (i - 1, i) if i > 0 else (i, i + 1)
            if last + 1 < len(bounds) and time.monotonic() < t0 + bounds[last + 1] / (2 * RATE):
                await asyncio.sleep(t0 + bounds[last + 1] / (2 * RATE) - time.monotonic())  # Successor not spoken yet.
            row['text'], row['request_ms'] = await request(f'merged-{first}-{last}', bounds[first], bounds[last + 1])
            row['merged_with'] = first if first != i else last
        row['done'] = time.monotonic()
        return row
    rows = await asyncio.gather(*(one(i) for i in range(len(bounds) - 1)))
    replaced = {r['merged_with'] for r in rows if 'merged_with' in r}
    return ' '.join(r['text'] for r in rows if r['chunk'] not in replaced), max(r['done'] for r in rows), rows


async def live(key, model, pcm, mode, t0, cut_at=None):
    """Returns (final_text, last_final_time, interrupted)."""
    finals, last = [], None
    setup = {'setup': {'model': f'models/{model}', 'generationConfig': {'responseModalities': ['TEXT']},
                       'inputAudioTranscription': {'languageCodes': LANGUAGES[mode]}}}
    cut_bytes = None if cut_at is None else int(len(pcm) * cut_at) // FRAME * FRAME
    async with websockets.connect(f'{LIVE}?key={key}', max_size=None) as socket:
        await socket.send(json.dumps(setup))
        if 'setupComplete' not in json.loads(await socket.recv()): raise RuntimeError('Live setup was not acknowledged')

        async def send():
            for offset in range(0, len(pcm), FRAME):
                delay = t0 + (offset + FRAME) / (2 * RATE) - time.monotonic()
                if delay > 0: await asyncio.sleep(delay)
                if cut_bytes is not None and offset >= cut_bytes:
                    await socket.close(); return True  # Simulated connection loss.
                await socket.send(json.dumps({'realtimeInput': {'audio': {'data': base64.b64encode(pcm[offset:offset + FRAME]).decode(), 'mimeType': f'audio/pcm;rate={RATE}'}}}))
            await socket.send(json.dumps({'realtimeInput': {'audioStreamEnd': True}}))
            return False

        sender = asyncio.create_task(send())
        stop = t0 + len(pcm) / (2 * RATE)
        try:
            while True:
                quiet = 5 if time.monotonic() > stop else 60
                message = json.loads(await asyncio.wait_for(socket.recv(), timeout=quiet))
                content = message.get('serverContent', {})
                if 'inputTranscription' in content:
                    finals.append(content['inputTranscription'].get('text', '')); last = time.monotonic()
        except (asyncio.TimeoutError, websockets.ConnectionClosed):
            pass
        interrupted = await sender
    return ' '.join(f.strip() for f in finals if f.strip()), last, interrupted


async def trial(a, key, clip, method, iteration, out):
    with wave.open(clip['wav'], 'rb') as audio: pcm = audio.readframes(audio.getnframes())
    bounds = cuts(pcm); duration = len(pcm) / (2 * RATE)
    scratch = out / f'{clip["id"]}-{method}-{iteration}'; scratch.mkdir()
    t0 = time.monotonic(); stop = t0 + duration
    row = {'clip': clip['id'], 'method': method, 'iteration': iteration, 'audio_s': round(duration, 1), 'chunks': len(bounds) - 1}
    try:
        if method == 'chunked':
            text, done, chunks = await batch_chunks(key, a.batch_model, pcm, clip['mode'], bounds, t0, scratch, a.thinking)
            row['chunk_rows'] = [{k: v for k, v in c.items() if k not in ('text', 'done')} for c in chunks]
        else:
            text, done, interrupted = await live(key, a.live_model, pcm, clip['mode'], t0, a.cut_at if method == 'live-cut' else None)
            row['interrupted'] = interrupted
            if interrupted:  # Fallback: the whole recording goes through chunked batch requests.
                row['cut_at_s'] = round(duration * a.cut_at, 1)
                text, done, chunks = await batch_chunks(key, a.batch_model, pcm, clip['mode'], bounds, t0, scratch, a.thinking)
                row['chunk_rows'] = [{k: v for k, v in c.items() if k not in ('text', 'done')} for c in chunks]
            elif done is None: raise RuntimeError('Live returned no final text')
        hyp, ref = tokens(text), tokens(clip['reference'])
        row.update(stop_to_text_ms=round((done - stop) * 1000), wer=round(wer(clip['reference'], text), 4), word_ratio=round(len(hyp) / len(ref), 3))
        (scratch / 'text.txt').write_text(text)
    except Exception as error:  # Never echo URLs or bodies: the key is in the live URL.
        row['error'] = type(error).__name__ + (f' {getattr(getattr(error, "response", None), "status_code", "")}' if hasattr(error, 'response') else '') + ('' if 'key=' in str(error) else f': {str(error)[:200]}')
    print(json.dumps(row), flush=True)
    return row


async def main_async(a, key):
    out = private_path(a.output); out.mkdir(parents=True, exist_ok=False)
    clips = {c['id']: c for c in json.loads(private_path(a.suite).read_text())['clips']}
    jobs = [trial(a, key, clips[c], m, i, out) for i in range(a.iterations) for c in a.clips for m in a.methods]
    rows = await asyncio.gather(*jobs)
    (out / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--suite', type=Path, required=True); p.add_argument('--output', type=Path, required=True)
    p.add_argument('--clips', nargs='+', required=True); p.add_argument('--methods', nargs='+', default=['chunked', 'live', 'live-cut'])
    p.add_argument('--iterations', type=int, default=1); p.add_argument('--cut-at', type=float, default=0.6)
    p.add_argument('--batch-model', default='gemini-3.8-flash'); p.add_argument('--thinking', choices=['default', 'off'], default='default'); p.add_argument('--live-model', default='gemini-3.5-transcribe-live')
    a = p.parse_args()
    key = os.environ.pop('GEMINI_API_KEY', None)
    if not key: raise SystemExit('Set GEMINI_API_KEY for this command only')
    asyncio.run(main_async(a, key))


if __name__ == '__main__':
    os.umask(0o077)
    main()
