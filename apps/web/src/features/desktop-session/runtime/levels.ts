import workletUrl from './level-processor.js?worker&url';

export interface Meter { dispose(): Promise<void> }
export async function meterStream(stream: MediaStream, publish: (rms: number, peak: number) => void): Promise<Meter> {
  const context = new AudioContext();
  let source: MediaStreamAudioSourceNode | undefined;
  let node: AudioWorkletNode | undefined;
  let disposed = false;
  let last = -Infinity;
  const dispose = async () => {
    disposed = true;
    source?.disconnect();
    if (node) { node.port.onmessage = null; node.port.close(); node.disconnect(); }
    await context.close();
  };
  try {
    await context.audioWorklet.addModule(workletUrl);
    source = context.createMediaStreamSource(stream);
    node = new AudioWorkletNode(context, 'writer-level', { numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1] });
    node.port.onmessage = ({ data }: MessageEvent<{ rms: number; peak: number }>) => {
      if (disposed) return;
      const now = performance.now();
      if (now - last >= 50 && Number.isFinite(data.rms) && Number.isFinite(data.peak)) {
        last = now;
        publish(Math.max(0, Math.min(data.rms, data.peak, 1)), Math.max(0, Math.min(data.peak, 1)));
      }
      node?.port.postMessage('consumed');
    };
    source.connect(node); node.connect(context.destination);
    await context.resume();
    return { dispose };
  } catch (error) { await dispose(); throw error; }
}
