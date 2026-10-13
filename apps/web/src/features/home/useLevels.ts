import { useEffect, useState } from 'react';
import type { SessionOwner } from '../desktop-session/runtime/owner';
import { meterStream } from '../desktop-session/runtime/levels';

const idle = (bars: number) => new Array<number>(bars).fill(0);

/** Display -60…0 dBFS: ordinary speech must not round down to the 2px floor.
 * This is display scaling only; audio and telemetry retain their original gain. */
export function displayLevel(rms: number) {
  return Number.isFinite(rms) && rms > 0 ? Math.max(0, Math.min(1, (20 * Math.log10(rms) + 60) / 60)) : 0;
}

/** Missing telemetry clears the meter; it does not diagnose a failed microphone. */
export function useLevels(recording: boolean, owner: SessionOwner | undefined, stream: MediaStream | undefined, bars = 24) {
  const [levels, setLevels] = useState<number[]>(() => idle(bars));
  useEffect(() => {
    setLevels(idle(bars));
    if (!recording) return;
    let last = performance.now();
    let disposed = false;
    const push = (rms: number) => {
      if (disposed) return;
      last = performance.now();
      setLevels(previous => [...previous.slice(1), displayLevel(rms)]);
    };
    const timer = window.setInterval(() => {
      if (performance.now() - last > 400) setLevels(idle(bars));
    }, 100);
    let unsubscribe: (() => void) | undefined;
    let meter: { dispose(): Promise<void> } | undefined;
    if (owner) unsubscribe = owner.subscribe(event => { if (event.kind === 'level') push(event.rms); });
    else if (stream) void meterStream(stream, push)
      .then(value => { if (disposed) void value.dispose(); else meter = value; })
      .catch(() => { /* Capture and saving remain independent of meter support. */ });
    return () => { disposed = true; clearInterval(timer); unsubscribe?.(); void meter?.dispose(); };
  }, [recording, owner, stream, bars]);
  return levels;
}
