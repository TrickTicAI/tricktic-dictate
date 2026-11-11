#!/usr/bin/env python3
"""Windows side of the personal scorecard (MW-121). Run with `python -I`.

Reads the installed TrickTic Dictate profile and decodes saved clips on the
local GPU through the packaged, authenticated whisper-server, with the same
request fields Writer sends. Nothing leaves this machine: results are written
to a private work folder that scripts/bench/scorecard.py copies back over SSH.

  list WORK                      clips with their delivered raw and polished text
  replay WORK MANIFEST CONFIGS   decode manifest clips with each configuration
  references-pull WORK           copy the app's saved references
  references-push WORK FILE      write drafted references (never over a user's)
"""
import json
import os
import struct
import subprocess
import sys
import threading
import time
import urllib.request
import uuid

APP = os.path.join(os.environ.get("LOCALAPPDATA", ""), "com.tricktic.dictate")
DATA = os.path.join(APP, "Data")
STATE = os.path.join(APP, "State")
SERVER = os.path.join(os.environ.get("LOCALAPPDATA", ""), "TrickTic Dictate", "runtime", "whisper", "whisper-server.exe")
TEMPERATURE_INC = "0.5"  # apps/api/src/local_stt/mod.rs TEMPERATURE_INC


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def save(path, value):
    temp = f"{path}.{uuid.uuid4().hex}.tmp"
    with open(temp, "w", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False)
    os.replace(temp, path)


def clips():
    """The latest successful transcription of every recording, with its polish."""
    latest = {}
    for name in os.listdir(os.path.join(DATA, "jobs")):
        if not name.endswith(".json"):
            continue
        job = load(os.path.join(DATA, "jobs", name))
        status = job["status"]
        if status.get("state") != "succeeded":
            continue
        options = job["request"]["options"]
        recording = job["request"]["recording_id"]
        transcript = status["result"]["transcript"]
        row = {
            "job": job["id"], "rec": recording, "t": job["created_at_ms"],
            "ms": (job.get("audio_preparation") or {}).get("input_ms"),
            "model": options["selection"]["model"], "lang": options.get("input_language"),
            "hints": options.get("vocabulary", {}).get("hints", []),
            "raw": transcript["raw_text"],
            # The exact input Writer recognized; absent once the audio was deleted.
            "audio": os.path.exists(os.path.join(DATA, "recordings", recording, "prepared.pcm")),
        }
        mode = os.path.join(STATE, "modes", job["id"], f"{job['id']}.json")
        if os.path.exists(mode):
            attempt = load(mode)
            diagnostics = attempt.get("diagnostics") or {}
            row.update(
                kind=(attempt.get("profile") or {}).get("kind"),
                polished=attempt["state"].get("text") if attempt["state"].get("state") == "succeeded" else None,
                polish_ms=diagnostics.get("processing_ms"),
                original_used=diagnostics.get("original_used"),
                s2d=(diagnostics.get("delivery") or {}).get("stop_to_delivery_ms"),
                app=(diagnostics.get("delivery") or {}).get("app"),
            )
        if recording not in latest or latest[recording]["t"] < row["t"]:
            latest[recording] = row
    return sorted(latest.values(), key=lambda r: r["t"])


def wav(pcm):
    return (b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt "
            + struct.pack("<IHHIIHH", 16, 1, 1, 16000, 32000, 2, 16)
            + b"data" + struct.pack("<I", len(pcm)) + pcm)


def short_profile(pcm_bytes):
    """Writer's short-window encoder context (local_stt WhisperProfile)."""
    size = pcm_bytes + 44
    if size > 15 * 32000 + 80:
        return 0, False
    return (512 if 1500 * 32 <= size <= 9500 * 32 + 80 else 768), True


def speechless(pcm):
    """The energy half of Writer's speech gate (local_stt speech::speechless):
    every 16 ms frame below -54 dBFS RMS. The app also requires no periodic
    voice structure and a low Earshot score, so this skips at most what it skips."""
    if not any(pcm):
        return True
    for start in range(0, len(pcm) - 1, 512):
        frame = pcm[start:start + 512]
        samples = struct.unpack(f"<{len(frame) // 2}h", frame[:len(frame) // 2 * 2])
        if sum(v * v for v in samples) >= len(samples) * 64 ** 2:
            return False
    return True


def sentence(hints):
    """Hints as one short natural sentence instead of a comma list."""
    if not hints:
        return ""
    listed = hints[0] if len(hints) == 1 else ", ".join(hints[:-1]) + " and " + hints[-1]
    return f"I'm talking about {listed}."


class Worker:
    """The packaged whisper-server, started with a fresh capability on stdin."""

    def __init__(self, model, log, server=SERVER, vad_model=None):
        self.capability = uuid.uuid4().hex + uuid.uuid4().hex
        instance = uuid.uuid4().hex + uuid.uuid4().hex
        extra = ["--vad-model", vad_model] if vad_model else []
        self.process = subprocess.Popen(
            [server, "-m", model, "-l", "en", "--host", "127.0.0.1", "--port", "0", "-t", "4", *extra],
            cwd=os.path.dirname(server), stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.process.stdin.write((self.capability + "\n" + instance + "\n").encode())
        self.process.stdin.close()
        self.port = None
        while self.port is None:
            line = self.process.stderr.readline().decode(errors="replace")
            if not line:
                raise SystemExit("whisper-server exited before listening")
            log.write(line.replace(self.capability, "[c]").replace(instance, "[i]"))
            if line.startswith("writer: private listener "):
                self.port = int(line.split()[-1])
        threading.Thread(target=lambda: [None for _ in self.process.stderr], daemon=True).start()

    def decode(self, pcm, config, hints):
        """Text and, with `segments`, whisper's segments (start, end, no-speech probability)."""
        replica = config.get("replica", True)
        audio_ctx, no_timestamps = short_profile(len(pcm)) if replica else (0, False)
        # "timestamped": segments only where Writer already decodes with
        # timestamps (clips over 15 s), so the decode itself is unchanged.
        segments = config.get("segments", False)
        if segments == "timestamped":
            segments = not no_timestamps
        elif segments:
            no_timestamps = False
        if "prompt_text" in config:
            prompt = config["prompt_text"]
        elif not config.get("prompt", True):
            prompt = ""
        elif config.get("prompt_style") == "sentence":
            prompt = sentence(hints)
        else:
            prompt = ", ".join(hints)
        fields = {
            "language": config.get("language", "en"), "response_format": "verbose_json" if segments else "json",
            "translate": "false", "temperature": "0.0", "best_of": "2", "beam_size": str(config.get("beam", -1)),
            "temperature_inc": str(config.get("temperature_inc", TEMPERATURE_INC)),
            "carry_initial_prompt": "true", "no_language_probabilities": "true", "prompt": prompt,
            "audio_ctx": str(audio_ctx), "no_timestamps": "true" if no_timestamps else "false",
        }
        if config.get("vad_model"):
            fields["vad"] = "true"
            for key in ("vad_threshold", "vad_speech_pad_ms", "vad_min_silence_duration_ms"):
                if key in config:
                    fields[key] = str(config[key])
        boundary = uuid.uuid4().hex
        body = b"".join(f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode() for k, v in fields.items())
        body += (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="r.wav"\r\n'
                 f"Content-Type: audio/wav\r\n\r\n").encode() + wav(pcm) + f"\r\n--{boundary}--\r\n".encode()
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.port}/inference", body,
            {"Content-Type": f"multipart/form-data; boundary={boundary}", "Authorization": "Bearer " + self.capability})
        with urllib.request.urlopen(request, timeout=600) as response:
            body = json.load(response)
        if not segments:
            return body.get("text", ""), None
        return body.get("text", ""), [{"text": s.get("text", ""), "start": s.get("start"), "end": s.get("end"),
                                       "no_speech": s.get("no_speech_prob"), "logprob": s.get("avg_logprob")}
                                      for s in body.get("segments", [])]

    def close(self):
        self.process.kill()


def replay(work, manifest, configs):
    rows = {r["job"]: r for r in clips()}
    out = {}
    for config in configs:
        name = config["name"]
        result = {}
        if config.get("source") == "recorded":
            # Today's delivered text, exactly as the app produced it.
            for clip in manifest["clips"]:
                row = rows.get(clip["job"]) or {}
                result[clip["id"]] = {"text": row.get("raw", ""), "ms": None, "polished": row.get("polished"),
                                      "polish_ms": row.get("polish_ms"), "original_used": row.get("original_used"),
                                      "s2d_ms": row.get("s2d")}
            out[name] = {"config": config, "clips": result}
            continue
        # An installed model, or one staged in the work folder for comparison.
        model = next((p for p in (config["model"], os.path.join(APP, "Models", config["model"]), os.path.join(work, config["model"]))
                      if os.path.isabs(p) and os.path.exists(p)), None)
        if model is None:
            raise SystemExit(f"Model {config['model']} is neither installed nor staged in {work}.")
        staged = lambda key: config.get(key) and next(
            (p for p in (config[key], os.path.join(work, config[key])) if os.path.isabs(p) and os.path.exists(p)), None)
        server = staged("server") or SERVER
        vad_model = staged("vad_model")
        if config.get("vad_model") and not vad_model:
            raise SystemExit(f"VAD model {config['vad_model']} is not staged in {work}.")
        with open(os.path.join(work, f"server-{name}.log"), "w", encoding="utf-8") as log:
            worker = Worker(model, log, server, vad_model)
            try:
                for clip in manifest["clips"]:
                    with open(os.path.join(DATA, "recordings", clip["id"], "prepared.pcm"), "rb") as handle:
                        pcm = handle.read()
                    started = time.perf_counter()
                    if config.get("gate", True) and speechless(pcm):
                        result[clip["id"]] = {"text": "", "ms": 0, "gated": True}
                        continue
                    hints = clip.get("hints", [])
                    if "hints" in config:
                        # Replaces the clip's own list: a fixed list, or one chosen per clip.
                        hints = config["hints"].get(clip["id"], []) if isinstance(config["hints"], dict) else config["hints"]
                    try:
                        text, segments = worker.decode(pcm, config, hints)
                        result[clip["id"]] = {"text": text.strip(), "ms": int((time.perf_counter() - started) * 1000)}
                        if segments is not None:
                            result[clip["id"]]["segments"] = segments
                    except Exception as error:  # one failed clip is reported, not fatal
                        result[clip["id"]] = {"text": "", "ms": None, "error": type(error).__name__}
            finally:
                worker.close()
        out[name] = {"config": config, "clips": result}
        print(name, "done", flush=True)
    return out


def references_pull():
    root = os.path.join(DATA, "references")
    if not os.path.isdir(root):
        return []
    return [load(os.path.join(root, n)) for n in sorted(os.listdir(root)) if n.endswith(".json")]


def references_push(drafts):
    """Drafts never replace a reference the maintainer saved."""
    root = os.path.join(DATA, "references")
    os.makedirs(root, exist_ok=True)
    written = kept = 0
    for draft in drafts:
        path = os.path.join(root, f"{draft['recording_id']}.json")
        if os.path.exists(path) and load(path).get("source", "user") == "user":
            kept += 1
            continue
        save(path, draft)
        written += 1
    return {"written": written, "kept_user": kept}


def main():
    command, work = sys.argv[1], sys.argv[2]
    os.makedirs(work, exist_ok=True)
    if command == "list":
        save(os.path.join(work, "clips.json"), clips())
    elif command == "replay":
        save(os.path.join(work, "replay.json"), replay(work, load(sys.argv[3]), load(sys.argv[4])))
    elif command == "references-pull":
        save(os.path.join(work, "references.json"), references_pull())
    elif command == "references-push":
        print(json.dumps(references_push(load(sys.argv[3]))))
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
