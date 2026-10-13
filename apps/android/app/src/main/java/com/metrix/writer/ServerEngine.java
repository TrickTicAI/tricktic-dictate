package com.metrix.writer;

import org.json.*;
import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

/** Saved-file server engine. No Activity, UI, provider credentials, or capture ownership. */
public final class ServerEngine implements TranscriptionEngine {
    private final SessionStore store;
    private final MobileConnection mobile;
    private String origin;
    private volatile HttpURLConnection activeConnection;
    private volatile boolean cancelled;
    public void cancel() { cancelled = true; if (activeConnection != null) activeConnection.disconnect(); }

    public ServerEngine(SessionStore store, MobileConnection mobile) { this.store = store; this.mobile = mobile; }
    public String execute(JSONObject item, String base) {
        String message;
        try {
            if (cancelled) throw new InterruptedIOException();
            origin = mobile.normalize(base);
            if (!origin.equals(base)) throw new IOException("Pending endpoint must be canonical");
            String id = item.getString("id");
            HttpURLConnection existing = connection(base + "/api/v1/recordings/" + id, "GET");
            try { int code = existing.getResponseCode(); if (code == 404) upload(item, base); else { JSONObject retained = response(existing).getJSONObject("data"); JSONObject meta = retained.getJSONObject("metadata");
                    for (String key : new String[]{"id", "created_at_ms", "mime_type", "byte_length", "duration_ms", "input_language", "capture_state"}) if (!meta.get(key).toString().equals(item.get(key).toString())) throw new IOException("Server metadata does not match saved session");
                    String state = retained.getJSONObject("audio").getString("state");
                    if (state.equals("uploading")) upload(item, base);
                    else if (!state.equals("saved")) throw new IOException("Server audio is not available"); } } finally { existing.disconnect(); }
            JSONObject pending = item.getJSONObject("pending");
            JSONObject transportTiming = item.optJSONObject("pending_timing");
            if (transportTiming != null) transportTiming.put("cloud_request_started_elapsed_ms", android.os.SystemClock.elapsedRealtime());
            JSONObject job = jsonRequest(base + "/api/v1/transcriptions", new JSONObject().put("api_version","1").put("data",pending)).getJSONObject("data");
            long deadline = android.os.SystemClock.elapsedRealtime()+300000;
            while (Arrays.asList("queued","transcribing").contains(job.getJSONObject("status").getString("state"))) {
                if (android.os.SystemClock.elapsedRealtime()>deadline) throw new IOException("Still processing; retry resumes the saved request");
                Thread.sleep(100); job = jsonRequest(base + "/api/v1/jobs/" + job.getString("id"), null).getJSONObject("data");
            }
            if (cancelled) throw new InterruptedIOException();
            JSONObject request = job.getJSONObject("request");
            if (!sameJson(request, pending)) throw new IOException("Result does not match the pending request");
            String terminal = job.getJSONObject("status").getString("state");
            if (!Arrays.asList("succeeded", "failed").contains(terminal)) throw new IOException("Unexpected job state");
            if (terminal.equals("succeeded")) job.getJSONObject("status").getJSONObject("result").getJSONObject("transcript").getString("raw_text");
            JSONObject timing = item.optJSONObject("pending_timing");
            if (timing != null) {
                long resultAt = android.os.SystemClock.elapsedRealtime();
                timing.put("cloud_request_finished_elapsed_ms", resultAt);
                timing.put("job_id", job.getString("id")).put("result_elapsed_ms", resultAt);
                if (timing.has("request_elapsed_ms")) timing.put("request_to_result_ms", resultAt - timing.getLong("request_elapsed_ms"));
                if (timing.has("capture_stop_elapsed_ms")) timing.put("stop_to_result_ms", resultAt - timing.getLong("capture_stop_elapsed_ms"));
                job.put("client_timing", timing); item.put("last_timing", timing); item.remove("pending_timing");
            }
            job.put("server_origin", base).put("delivery_choice", item.optString("pending_delivery", "raw"));
            item.remove("pending_delivery");
            item.getJSONArray("attempts").put(job); item.remove("pending"); item.remove("pending_server");
            JSONObject state = job.getJSONObject("status"); message = state.getString("state").equals("succeeded") ? "Transcription succeeded; original saved separately" : "Transcription failed; audio and previous originals retained. " + state.getJSONObject("error").getString("message");
            if (state.getString("state").equals("succeeded")) item.put("delivery", new JSONObject().put("state", "pending"));
            item.put("last_status",message); store.saveRaw(item, job);
        } catch (Exception e) { try { item.put("pending_latency_failed", true); store.save(item); } catch (Exception ignored) { }
            android.util.Log.w("WriterNetwork", "Request incomplete: " + e.getClass().getSimpleName()); message = MobileConnection.explain(e) + " Audio and exact pending request are saved; retry to resume."; }
        return message;
    }
    // JSON member order is not a protocol property. Compare the entire immutable request recursively.
    private static boolean sameJson(Object a, Object b) throws JSONException {
        if (a instanceof JSONObject && b instanceof JSONObject) {
            JSONObject x = (JSONObject)a, y = (JSONObject)b;
            if (x.length() != y.length()) return false;
            Iterator<String> keys = x.keys();
            while (keys.hasNext()) { String key = keys.next(); if (!y.has(key) || !sameJson(x.get(key), y.get(key))) return false; }
            return true;
        }
        if (a instanceof JSONArray && b instanceof JSONArray) {
            JSONArray x = (JSONArray)a, y = (JSONArray)b;
            if (x.length() != y.length()) return false;
            for (int i = 0; i < x.length(); i++) if (!sameJson(x.get(i), y.get(i))) return false;
            return true;
        }
        return a.equals(b);
    }
    private HttpURLConnection connection(String url, String method) throws Exception {
        if (cancelled) throw new InterruptedIOException();
        if (!url.startsWith(origin + "/api/v1/")) throw new IOException("Endpoint mismatch");
        HttpURLConnection c = mobile.open(origin, url.substring(origin.length()), method); activeConnection = c; return c;
    }
    private JSONObject response(HttpURLConnection connection) throws Exception { return MobileConnection.response(connection); }

    private JSONObject jsonRequest(String url, JSONObject body) throws Exception {
        HttpURLConnection c = connection(url, body == null ? "GET" : "POST");
        try { if (body != null) { c.setDoOutput(true); c.setRequestProperty("Content-Type","application/json"); try(OutputStream out=c.getOutputStream()) { out.write(body.toString().getBytes(StandardCharsets.UTF_8)); } } return response(c); } finally { c.disconnect(); }
    }
    private void upload(JSONObject item, String base) throws Exception {
        JSONObject timing = item.optJSONObject("pending_timing");
        if (timing != null) timing.put("upload_started_elapsed_ms", android.os.SystemClock.elapsedRealtime());
        JSONObject metadata = new JSONObject(); for(String key: new String[]{"id","created_at_ms","mime_type","byte_length","duration_ms","input_language","capture_state"}) metadata.put(key,item.get(key));
        String boundary = "writer-"+UUID.randomUUID(); HttpURLConnection c = connection(base+"/api/v1/recordings", "POST"); c.setDoOutput(true); c.setChunkedStreamingMode(8192); c.setRequestProperty("Content-Type","multipart/form-data; boundary="+boundary);
        try {
            try(OutputStream out=c.getOutputStream(); InputStream in=new FileInputStream(store.audio(item))) {
                out.write(("--"+boundary+"\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n"+new JSONObject().put("api_version","1").put("data",metadata)+"\r\n--"+boundary+"\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"recording.wav\"\r\nContent-Type: audio/wav\r\n\r\n").getBytes(StandardCharsets.UTF_8));
                byte[] bytes=new byte[8192]; int n; while((n=in.read(bytes))!=-1) out.write(bytes,0,n); out.write(("\r\n--"+boundary+"--\r\n").getBytes(StandardCharsets.UTF_8));
            } response(c);
            if (timing != null) timing.put("upload_finished_elapsed_ms", android.os.SystemClock.elapsedRealtime());
        } finally { c.disconnect(); }
    }
}
