package com.metrix.writer;

import android.media.*;
import org.json.*;
import java.io.*;

/** One native owner. Stop drains complete buffered frames; failures preserve the saved prefix. */
public final class AudioCapture {
    public interface Completion { void finished(JSONObject item, String error); }
    public interface Source extends AutoCloseable {
        void start() throws Exception;
        int read(byte[] bytes, int length) throws Exception;
        int capacity();
        boolean silenced();
        int route();
        void close();
    }
    public interface SourceFactory { Source open() throws Exception; }
    private static final class Microphone implements Source {
        private final AudioRecord recorder;
        Microphone() throws IOException {
            int minimum = AudioRecord.getMinBufferSize(16000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT);
            if (minimum <= 0) throw new IOException("Unsupported microphone format");
            try { recorder = new AudioRecord(MediaRecorder.AudioSource.MIC, 16000, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, Math.max(6400, minimum)); }
            catch (SecurityException e) { throw new IOException("Microphone permission unavailable", e); }
            if (recorder.getState() != AudioRecord.STATE_INITIALIZED) { recorder.release(); throw new IOException("Microphone initialization failed"); }
        }
        public void start() { recorder.startRecording();
            if (recorder.getRecordingState() != AudioRecord.RECORDSTATE_RECORDING) throw new IllegalStateException("Microphone did not start"); }
        public int read(byte[] bytes, int length) { return recorder.read(bytes, 0, length, AudioRecord.READ_NON_BLOCKING); }
        public int capacity() { return recorder.getBufferSizeInFrames() * 2; }
        public boolean silenced() { AudioRecordingConfiguration c = recorder.getActiveRecordingConfiguration(); return c != null && c.isClientSilenced(); }
        public int route() { AudioDeviceInfo d = recorder.getRoutedDevice(); return d == null ? 0 : d.getId(); }
        public void close() { try { recorder.stop(); } catch (Exception ignored) {} recorder.release(); }
    }
    private final SessionStore store;
    private final SourceFactory sources;
    private volatile boolean capturing, active, recording;
    private volatile float level;
    private volatile String interruption;
    public AudioCapture(SessionStore store) { this(store, Microphone::new); }
    // Native source boundary permits deterministic drain/contention/route tests without a debug capture backdoor.
    public AudioCapture(SessionStore store, SourceFactory sources) { this.store = store; this.sources = sources; }
    public boolean isActive() { return active; }
    public boolean isCapturing() { return capturing; }
    public boolean isRecording() { return recording; }
    public float getLevel() { return level; }
    public synchronized void start(JSONObject item, Completion completion) { start(item, completion, () -> {}); }
    public synchronized void start(JSONObject item, Completion completion, Runnable ready) {
        if (active) throw new IllegalStateException("Capture already active");
        store.setActiveCaptureId(item.optString("id"));
        interruption = null; level = 0; recording = false; capturing = true; active = true;
        new Thread(() -> capture(item, completion, ready), "writer-capture").start();
    }
    public synchronized void stop() { capturing = false; }
    public synchronized void interrupt(String reason) { if (active && interruption == null) interruption = reason; capturing = false; }
    private void capture(JSONObject item, Completion completion, Runnable ready) {
        String error = null;
        try (Source recorder = sources.open(); RandomAccessFile wav = new RandomAccessFile(store.audio(item), "rw")) {
            recorder.start();
            JSONObject timing = item.optJSONObject("startup_timing");
            if (timing == null) { timing = new JSONObject(); item.put("startup_timing", timing); }
            timing.put("microphone_started_elapsed_ms", android.os.SystemClock.elapsedRealtime());
            boolean first = true;
            byte[] buffer = new byte[3200]; int drainBytes = -1, route = 0;
            while (true) {
                if (recorder.silenced()) throw new IOException("Microphone unavailable or silenced by Android");
                int nextRoute = recorder.route();
                if (route != 0 && nextRoute != route) throw new IOException("Microphone route changed");
                if (nextRoute != 0) route = nextRoute;
                // Stop snapshots a finite ring-buffer budget, never an arbitrary dictation duration.
                if (!capturing && drainBytes < 0) drainBytes = recorder.capacity();
                if (drainBytes == 0) break;
                int count = recorder.read(buffer, drainBytes < 0 ? buffer.length : Math.min(buffer.length, drainBytes));
                if (count == 0) { if (!capturing) break; Thread.sleep(10); continue; }
                if (count < 0) throw new IOException("Microphone read failed");
                if ((count & 1) != 0) throw new IOException("Incomplete microphone frame");
                if (drainBytes > 0) drainBytes -= count;
                if (wav.length() + count - 8 > 0xffffffffL) throw new IOException("WAV size limit reached");
                if (first) timing.put("first_audio_read_elapsed_ms", android.os.SystemClock.elapsedRealtime());
                wav.seek(wav.length()); wav.write(buffer, 0, count);
                SessionStore.header(wav, wav.length() - 44); wav.getFD().sync();
                if (first) {
                    timing.put("first_audio_saved_elapsed_ms", android.os.SystemClock.elapsedRealtime());
                    first = false; recording = true; ready.run();
                }
                double sum = 0;
                for (int i = 0; i < count; i += 2) { double sample = (short)((buffer[i] & 255) | (buffer[i+1] << 8)) / 32768.0; sum += sample * sample; }
                level = (float)Math.sqrt(sum / (count / 2));
            }
        } catch (SecurityException e) { error = "Microphone permission unavailable. Saved audio retained.";
        } catch (Exception e) { error = "Recording interrupted (microphone, route or storage failure). Saved audio retained."; }
        finally {
            recording = false; capturing = false; level = 0;
            if (interruption != null) error = interruption;
            try { store.finishCapture(item, error); }
            catch (Exception e) { error = "Audio/metadata save failed. Reopen " + ProductKt.PRODUCT_NAME + " to recover saved frames."; }
            // Callback finalizes discard/service ownership before another capture can begin.
            try { completion.finished(item, error); }
            finally { store.setActiveCaptureId(null); active = false; }
        }
    }
}
