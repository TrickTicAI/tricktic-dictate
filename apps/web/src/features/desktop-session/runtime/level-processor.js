// Runs on the audio rendering thread. One outstanding summary, never raw audio.
class WriterLevelProcessor extends AudioWorkletProcessor {
  constructor() {
    super(); this.pending = false; this.frames = 0; this.sum = 0; this.peak = 0;
    this.port.onmessage = () => { this.pending = false; };
  }
  process(inputs) {
    const channel = inputs[0]?.[0];
    if (!channel) return true;
    for (const sample of channel) { this.sum += sample * sample; this.peak = Math.max(this.peak, Math.abs(sample)); }
    this.frames += channel.length;
    if (this.frames >= sampleRate / 20) {
      if (!this.pending) {
        this.pending = true;
        this.port.postMessage({ rms: Math.min(1, Math.sqrt(this.sum / this.frames)), peak: Math.min(1, this.peak) });
      }
      this.frames = 0; this.sum = 0; this.peak = 0;
    }
    // Output stays zero: metering must never play the microphone to speakers.
    return true;
  }
}
registerProcessor('writer-level', WriterLevelProcessor);
