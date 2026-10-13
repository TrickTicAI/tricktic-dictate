import { useEffect, useRef, useState } from 'react';
import { Download, Pause, Play } from 'lucide-react';
import { formatDuration } from '../../recorder';

const BARS = 96;
/* Decoding resamples the whole recording into memory. Ten minutes at 8 kHz is
   about 19 MB of samples; anything longer keeps a plain track instead. */
const DECODE_LIMIT_MS = 10 * 60_000;
const RATES = [1, 1.25, 1.5, 2, 0.75];
const peaksBySource = new Map<string, number[]>();

/**
 * The real shape of a saved recording, decoded here from the audio already on
 * this computer; nothing is uploaded to draw it. Null while decoding, for very
 * long recordings and when the format cannot be decoded.
 */
function usePeaks(src: string, durationMs: number): number[] | null {
  const [peaks, setPeaks] = useState<number[] | null>(() => peaksBySource.get(src) ?? null);
  useEffect(() => {
    const known = peaksBySource.get(src);
    if (known) { setPeaks(known); return; }
    setPeaks(null);
    if (!src || !(durationMs > 0) || durationMs > DECODE_LIMIT_MS || typeof OfflineAudioContext === 'undefined') return;
    let live = true;
    void (async () => {
      try {
        const encoded = await (await fetch(src)).arrayBuffer();
        const audio = await new OfflineAudioContext(1, 1, 8000).decodeAudioData(encoded);
        const samples = audio.getChannelData(0);
        const size = Math.max(1, Math.floor(samples.length / BARS));
        const raw = Array.from({ length: BARS }, (_, bar) => {
          let peak = 0;
          for (let index = bar * size, end = Math.min(samples.length, index + size); index < end; index += 2) {
            const value = Math.abs(samples[index]);
            if (value > peak) peak = value;
          }
          return peak;
        });
        const loudest = Math.max(...raw, 1e-4);
        // Square-root scaling, as on the floating panel: quiet speech stays visible.
        const shape = raw.map((value) => Math.sqrt(value / loudest));
        peaksBySource.set(src, shape);
        if (live) setPeaks(shape);
      } catch { /* An undecodable file still plays; it just draws a plain track. */ }
    })();
    return () => { live = false; };
  }, [src, durationMs]);
  return peaks;
}

/**
 * Playback for a saved recording, drawn on the theme tokens: play, the
 * waveform as the scrubber, elapsed time, speed and Save audio.
 *
 * The browser's own `<audio controls>` is the one object it paints in its own
 * colours, so it is replaced. The duration comes from the recording rather
 * than the media element, because a still-growing or repaired WAV can report
 * `Infinity` until it is fully buffered, and a scrubber that jumps once
 * metadata lands is worse than one that was right from the first frame.
 */
export function AudioPlayer({ src, durationMs, label, playRequested, onPlayed, onSave }: {
  src: string;
  durationMs: number;
  label: string;
  /** Set when something outside asked for this recording to start playing. */
  playRequested?: boolean;
  onPlayed?: () => void;
  onSave?: () => void;
}) {
  const element = useRef<HTMLAudioElement>(null);
  const [playing, setPlaying] = useState(false);
  const [at, setAt] = useState(0);
  const [rate, setRate] = useState(1);
  const peaks = usePeaks(src, durationMs);

  // A new recording resets the transport; the element is reused across sessions.
  useEffect(() => { setPlaying(false); setAt(0); }, [src]);
  useEffect(() => { if (element.current) element.current.playbackRate = rate; }, [rate, src]);

  // Playback asked for from elsewhere — the row's menu — starts once the saved
  // audio is actually readable, which is a moment later than the request.
  useEffect(() => {
    if (!playRequested || !src) return;
    const audio = element.current;
    if (!audio) return;
    void audio.play().catch(() => { /* An unreadable blob stays a stopped transport. */ });
    onPlayed?.();
  }, [playRequested, src, onPlayed]);

  const total = Number.isFinite(durationMs) && durationMs > 0 ? durationMs : 0;
  const fraction = total ? Math.min(1, at / total) : 0;
  const shape = peaks ?? new Array<number>(BARS).fill(0.18);
  const played = Math.round(fraction * shape.length);

  function toggle() {
    const audio = element.current;
    if (!audio) return;
    if (audio.paused) void audio.play().catch(() => setPlaying(false));
    else audio.pause();
  }

  function seek(event: React.ChangeEvent<HTMLInputElement>) {
    const audio = element.current;
    const next = Number(event.target.value);
    setAt(next);
    if (audio && Number.isFinite(next)) audio.currentTime = next / 1000;
  }

  return (
    <div className="player" data-decoded={peaks ? 'true' : 'false'}>
      <button
        type="button"
        className="player-toggle"
        aria-label={playing ? `Pause ${label}` : `Play ${label}`}
        onClick={toggle}
      >
        {playing ? <Pause size={16} aria-hidden /> : <Play size={16} aria-hidden />}
      </button>

      <div className="player-wave">
        <span className="player-bars" aria-hidden>
          {shape.map((value, index) => <i key={index} className={index < played ? 'played' : undefined} style={{ height: `${Math.max(8, Math.round(value * 100))}%` }} />)}
        </span>
        <input
          type="range"
          className="player-scrub"
          aria-label={`${label} position`}
          min={0}
          max={total || 1}
          step={100}
          value={Math.min(at, total || 1)}
          disabled={!total}
          onChange={seek}
        />
      </div>

      <span className="player-time mono">
        {formatDuration(at)}<span className="faint"> / {formatDuration(total)}</span>
      </span>
      <button
        type="button"
        className="button ghost small player-rate"
        aria-label={`Playback speed ${rate}×`}
        title="Playback speed"
        onClick={() => setRate(RATES[(RATES.indexOf(rate) + 1) % RATES.length])}
      >
        {rate}×
      </button>
      {onSave && <button type="button" className="button ghost small" onClick={onSave}><Download size={13} aria-hidden /> Save audio</button>}

      <audio
        ref={element}
        src={src}
        preload="metadata"
        aria-label={label}
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => { setPlaying(false); setAt(0); }}
        onTimeUpdate={(event) => setAt(event.currentTarget.currentTime * 1000)}
      />
    </div>
  );
}
