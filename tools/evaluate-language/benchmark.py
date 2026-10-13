#!/usr/bin/env python3
"""Archive and compare supplied language benchmark runs. No inference or capture."""
import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import shutil
import tempfile

from evaluate import ROOT, evaluate, normalized

DEVICES = ("windows_gpu", "phone", "cloud")
LANGUAGES = ("en", "he", "mixed")
STAGES = ("raw", "cleanup")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2,
                       allow_nan=False) + "\n").encode()


def private_path(path):
    path = Path(path).resolve()
    if path.is_relative_to(ROOT):
        raise ValueError("Keep audio, predictions and archived runs outside source")
    return path


def finite_ms(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def validate(manifest, run):
    if not manifest.get("fixture_version") or run.get("fixture_version") != manifest["fixture_version"]:
        raise ValueError("fixture_version must match the manifest")
    if not manifest.get("samples") or len({s["id"] for s in manifest["samples"]}) != len(manifest["samples"]):
        raise ValueError("manifest requires nonempty unique samples")
    for sample in manifest["samples"]:
        if sample["input_language"] not in LANGUAGES or sample.get("split") not in ("public", "synthetic") or not sample.get("tags"):
            raise ValueError("sample requires language, public/synthetic split and tags")
    if run.get("device") not in DEVICES:
        raise ValueError("device must be windows_gpu, phone or cloud")
    if not run.get("hardware") or not run.get("runtime"):
        raise ValueError("run requires hardware and runtime identity")
    stages = run.get("stages", {})
    if "raw" not in stages:
        raise ValueError("run requires raw stage metadata")
    if run.get("model") != stages["raw"].get("model") or run.get("model_revision") != stages["raw"].get("model_revision"):
        raise ValueError("legacy model metadata must identify the raw stage")
    for name, meta in stages.items():
        if name not in STAGES or not all(meta.get(k) for k in ("provider", "model", "model_revision")):
            raise ValueError("stage requires provider, model and model_revision")
        if not isinstance(meta.get("prompt"), str) or not isinstance(meta.get("decoding"), dict):
            raise ValueError("stage requires literal prompt and decoding settings (empty allowed)")
    for result in run["results"]:
        if not isinstance(result.get("raw_text"), str):
            raise ValueError("raw_text must be a string; never fabricate failed predictions")
        for stage, field in (("raw", "raw_text"), ("cleanup", "cleaned_text")):
            if field not in result:
                continue
            if stage not in stages or not isinstance(result[field], str):
                raise ValueError("prediction requires its stage metadata and string text")
            value = result.get("latency_ms", {}).get(stage)
            if run["kind"] == "audio_run" and not finite_ms(value):
                raise ValueError("audio-run stages require measured nonnegative latency_ms")
        if run["kind"] == "audio_run":
            if not result.get("codec") or not finite_ms(result.get("recording_duration_ms")):
                raise ValueError("audio-run result requires codec and recording_duration_ms")
            path = private_path(result["audio_path"])
            if digest(path.read_bytes()) != result.get("audio_sha256"):
                raise ValueError("audio hash mismatch")
            if "source_audio_sha256" in result:
                source = private_path(result["source_audio_path"])
                if digest(source.read_bytes()) != result["source_audio_sha256"] or not result.get("preparation"):
                    raise ValueError("codec/source variant requires verified source hash and preparation settings")
            sample = next(s for s in manifest["samples"] if s["id"] == result["id"])
            if sample.get("audio_sha256") and sample["audio_sha256"] != result.get("source_audio_sha256", result["audio_sha256"]):
                raise ValueError("result source does not match frozen fixture audio")
    return evaluate(manifest, run)


def report(manifest, run):
    scored = validate(manifest, run)
    samples = {s["id"]: s for s in manifest["samples"]}
    results = {r["id"]: r for r in run["results"]}
    groups = defaultdict(list)
    for row in scored["results"]:
        source, prediction = samples[row["id"]], results[row["id"]]
        row.update(split=source["split"], tags=source["tags"],
                   audio_sha256=prediction.get("audio_sha256"),
                   source_audio_sha256=prediction.get("source_audio_sha256", prediction.get("audio_sha256")),
                   raw_input_sha256=digest(prediction["raw_text"].encode()),
                   latency_ms=prediction.get("latency_ms", {}))
        for stage in STAGES:
            if stage in row:
                groups[(row["input_language"], source["split"], stage)].append(row)
    summary = []
    for (language, split, stage), rows in sorted(groups.items()):
        references = [samples[r["id"]].get("processed_expected_text", samples[r["id"]]["expected_text"])
                      if stage == "cleanup" else samples[r["id"]]["expected_text"] for r in rows]
        words = sum(len(normalized(s).split()) for s in references)
        chars = sum(len(normalized(s)) for s in references)
        summary.append({"language": language, "split": split, "stage": stage, "samples": len(rows),
                        "word_error_rate": sum(r[stage]["word_edits"] for r in rows) / words if words else None,
                        "character_error_rate": sum(r[stage]["normalized_character_edits"] for r in rows) / chars if chars else None,
                        "critical_failures": sum(bool(r[stage]["critical_errors"]) or r[stage].get("exact_values_preserved") is False for r in rows)})
    scored.update(fixture_version=manifest["fixture_version"], device=run["device"],
                  stages=run["stages"], summary=summary,
                  coverage=[{"language": lang, "device": device, "stage": stage,
                             "measured_samples": sum(r["input_language"] == lang and stage in r for r in scored["results"])
                             if run["kind"] == "audio_run" and device == run["device"] else 0,
                             "status": "measured" if run["kind"] == "audio_run" and device == run["device"] and any(r["input_language"] == lang and stage in r for r in scored["results"]) else "unmeasured"}
                            for lang in LANGUAGES for device in DEVICES for stage in STAGES])
    return scored


def scorer_bytes():
    return Path(__file__).read_bytes() + (Path(__file__).parent / "evaluate.py").read_bytes()


def archive(manifest_path, run_path, archive_root):
    """Content addressed, exclusive creation; existing archives are never rewritten."""
    manifest_bytes, run_bytes = Path(manifest_path).read_bytes(), Path(run_path).read_bytes()
    manifest, run = json.loads(manifest_bytes), json.loads(run_bytes)
    scored = report(manifest, run)
    hashes = {"fixtures_sha256": digest(manifest_bytes), "run_sha256": digest(run_bytes),
              "scorer_sha256": digest(scorer_bytes())}
    run_id = digest(encoded(hashes))
    root = private_path(archive_root)
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    target = root / run_id
    if target.exists():
        load_archive(target)
        return target
    scored.update(hashes, run_id=run_id)
    staging = Path(tempfile.mkdtemp(prefix=".pending-", dir=root))
    try:
        files = {"fixtures.json": manifest_bytes, "run.json": run_bytes,
                 "report.json": encoded(scored), "scorer.py.txt": scorer_bytes()}
        files["integrity.json"] = encoded({name: digest(data) for name, data in files.items()})
        for name, data in files.items():
            path = staging / name
            path.write_bytes(data)
            path.chmod(0o444)
        # mkdir is an exclusive reservation, including concurrent writers.
        target.mkdir(mode=0o700)
        for path in staging.iterdir():
            path.rename(target / path.name)
        target.chmod(0o555)
    finally:
        shutil.rmtree(staging)
    return target


def load_archive(path):
    path = Path(path)
    integrity = json.loads((path / "integrity.json").read_bytes())
    if set(integrity) != {"fixtures.json", "run.json", "report.json", "scorer.py.txt"}:
        raise ValueError("incomplete archive integrity receipt")
    for name, expected in integrity.items():
        if digest((path / name).read_bytes()) != expected:
            raise ValueError("archive integrity mismatch")
    result = json.loads((path / "report.json").read_bytes())
    hashes = {"fixtures_sha256": integrity["fixtures.json"], "run_sha256": integrity["run.json"],
              "scorer_sha256": integrity["scorer.py.txt"]}
    if any(result[k] != v for k, v in hashes.items()) or result["run_id"] != digest(encoded(hashes)) or path.name != result["run_id"]:
        raise ValueError("archive identity mismatch")
    return result


def compare(left_path, right_path):
    left, right = load_archive(left_path), load_archive(right_path)
    if left["fixtures_sha256"] != right["fixtures_sha256"] or left["scorer_sha256"] != right["scorer_sha256"] or left["kind"] != right["kind"]:
        raise ValueError("comparison requires identical fixtures, scorer and evidence kind")
    before, after = ({r["id"]: r for r in side["results"]} for side in (left, right))
    pairs, excluded = [], []
    for sample_id in sorted(before.keys() | after.keys()):
        a, b = before.get(sample_id), after.get(sample_id)
        for stage in STAGES:
            reason = None
            if not a or not b or stage not in a or stage not in b:
                reason = "missing stage or sample"
            elif a["source_audio_sha256"] != b["source_audio_sha256"]:
                reason = "different source audio"
            elif stage == "cleanup" and a["raw_input_sha256"] != b["raw_input_sha256"]:
                reason = "different raw input; cleanup effect cannot be isolated"
            if reason:
                excluded.append({"id": sample_id, "stage": stage, "reason": reason})
                continue
            pairs.append({"id": sample_id, "language": a["input_language"], "stage": stage,
                          "audio_sha256_before": a["audio_sha256"], "audio_sha256_after": b["audio_sha256"],
                          "word_edits_delta": b[stage]["word_edits"] - a[stage]["word_edits"],
                          "character_edits_delta": b[stage]["normalized_character_edits"] - a[stage]["normalized_character_edits"],
                          "critical_errors_before": a[stage]["critical_errors"], "critical_errors_after": b[stage]["critical_errors"],
                          "exact_values_before": a[stage].get("exact_values_preserved"), "exact_values_after": b[stage].get("exact_values_preserved"),
                          "latency_ms_before": a["latency_ms"].get(stage), "latency_ms_after": b["latency_ms"].get(stage)})
    return {"before": left["run_id"], "after": right["run_id"], "kind": left["kind"], "pairs": pairs, "excluded": excluded,
            "notes": ["Negative edit deltas mean fewer errors. Missing cells are unmeasured.",
                      "Codec experiments retain encoded hashes and must share a verified source audio hash.",
                      "No measured external-app correction effort or collected corrections; time saved is an estimate."]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    score = sub.add_parser("archive")
    score.add_argument("run", type=Path)
    score.add_argument("--fixtures", type=Path, required=True)
    score.add_argument("--archive-root", type=Path, required=True)
    diff = sub.add_parser("compare")
    diff.add_argument("before", type=Path)
    diff.add_argument("after", type=Path)
    diff.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.action == "archive":
        print(archive(args.fixtures, args.run, args.archive_root))
    else:
        result = compare(args.before, args.after)
        # Exclusive file creation prevents comparison receipts being overwritten.
        with private_path(args.output).open("xb") as output:
            output.write(encoded(result))
        print(args.output)


if __name__ == "__main__":
    main()
