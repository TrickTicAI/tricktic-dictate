#!/usr/bin/env python3
"""Personal dictation scorecard (MW-121).

One command replays a recognition configuration over the maintainer's gold clips on
the Windows GPU, scores it against his references and compares it with the
last accepted run, with a paired bootstrap interval:

  scorecard.py run --configs scripts/bench/scorecard-today.json
  scorecard.py accept <run>        make a run the comparison baseline
  scorecard.py score <run>...      rescore saved runs (after references change)
  scorecard.py score --against B <run>...   compare with run B instead of the accepted one
  scorecard.py derive <run> --tails S       drop a final segment that is only a known
                                   hallucination ("Thank you.") and starts in the last S
                                   seconds of the audio (needs a `segments` run)
  scorecard.py references pull     copy the app's references into the gold folder
  scorecard.py references push F   send drafted references to the app's History
  scorecard.py import F --name N   add runs decoded elsewhere ({config: {job: {text, ms}}})
  scorecard.py polish-plan --configs scripts/bench/scorecard-polish.json --raw <run>
  scorecard.py polish-run --exe <api test exe>   paid polish replay of the plan (MW-123)

The gold folder (default ~/.local/share/metrix-writer/eval/gold, override with
--gold or WRITER_GOLD_DIR) is private and outside the repository: manifest.json,
references.json, runs/ and accepted.json. Output is counts and rates only;
no transcript or reference text is printed.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "evaluate-language"))
import personal  # noqa: E402

WORKER = Path(__file__).with_name("scorecard-worker.py")
REMOTE = "$env:LOCALAPPDATA\\Temp\\mw-scorecard"


def gold_dir(value):
    path = Path(value or os.environ.get("WRITER_GOLD_DIR") or Path.home() / ".local/share/metrix-writer/eval/gold")
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    return path


def load(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def save(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temp = path.with_suffix(".tmp")
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False, indent=1)
    os.replace(temp, path)


def stratum(ms):
    return "short" if ms <= 15_000 else "medium" if ms <= 60_000 else "long"


def windows(script, host):
    """Runs PowerShell on the Windows host without echoing its output here."""
    import base64
    encoded = base64.b64encode(script.encode("utf-16-le")).decode()
    return subprocess.run(["ssh", host, f"powershell -NoProfile -EncodedCommand {encoded}"],
                          capture_output=True, text=True, check=False)


def copy_to(host, local, remote_name):
    subprocess.run(["scp", "-q", str(local), f"{host}:AppData/Local/Temp/mw-scorecard/{remote_name}"], check=True)


def copy_from(host, remote_name, local):
    subprocess.run(["scp", "-q", f"{host}:AppData/Local/Temp/mw-scorecard/{remote_name}", str(local)], check=True)


def worker(host, *args):
    prepared = windows(f'New-Item -ItemType Directory -Force "{REMOTE}" | Out-Null', host)
    if prepared.returncode:
        raise SystemExit("Could not reach the Windows host over SSH.")
    copy_to(host, WORKER, "scorecard-worker.py")
    quoted = " ".join(f'"{a}"' for a in args)
    done = windows(f'python -I "{REMOTE}\\scorecard-worker.py" {quoted} 2>&1', host)
    if done.returncode:
        # The worker prints no transcript text; its error tail is safe to show.
        raise SystemExit(f"Windows worker failed:\n{done.stdout[-2000:]}")
    return done.stdout


def references(gold):
    """Scorable references: gold, checked (or agreeing drafts) and fully resolved."""
    path = gold / "references.json"
    if not path.exists():
        raise SystemExit(f"No references in {gold}; run `scorecard.py references pull` first.")
    out = {}
    for item in load(path):
        if not item.get("gold") or item.get("needs_check"):
            continue
        tokens = personal.reference_words(item["text"])
        if tokens is not None:
            out[item["recording_id"]] = tokens
    return out


def terms(gold):
    path = gold / "terms.json"
    return [personal.words(t) for t in (load(path) if path.exists() else personal.PROJECT_TERMS)]


def score(gold, run):
    manifest = {c["id"]: c for c in load(gold / "manifest.json")["clips"]}
    refs, vocabulary = references(gold), terms(gold)
    scored = {}
    for clip_id, result in run["clips"].items():
        if clip_id not in refs or clip_id not in manifest or result.get("error"):
            continue
        clip = personal.score_clip(refs[clip_id], result.get("text", ""), vocabulary, result.get("polished"))
        clip["stratum"] = manifest[clip_id].get("stratum") or stratum(manifest[clip_id]["ms"])
        for key in ("ms", "s2d_ms", "polish_ms"):
            if result.get(key) is not None:
                clip[key] = result[key]
        scored[clip_id] = clip
    return scored


def percent(value):
    return "n/a" if value is None else f"{100 * value:.1f}%"


COMPARED = (("WER", "errors", "words"), ("term error rate", "term_misses", "terms"))
# Only when both runs carry a polish: they were given the same or comparable input.
COMPARED_POLISH = (("polished WER", "polished_errors", "words"), ("polished term error rate", "polished_term_misses", "terms"),
                   ("meaning changed", "meaning_changed", "one"))


def report(gold, name, against=None):
    run = load(gold / "runs" / f"{name}.json")
    scored = score(gold, run)
    total = personal.summary(scored)
    print(f"\n{name}: {total['clips']} clips, {total['words']} reference words")
    print(f"  WER {percent(total['wer'])}  (<=15 s {percent(total['wer_short'])}, 15-60 s {percent(total['wer_medium'])}, >60 s {percent(total['wer_long'])})")
    print(f"  project-term error rate {percent(total['term_error_rate'])} of {total['term_occurrences']} occurrences")
    print(f"  hallucinations {total['hallucinations']}, dropped content {total['dropped']}")
    if "polished" in total:
        print(f"  polish: WER {percent(total['polished_wer'])}, term fixes {total['polish_term_fixes']}, breaks {total['polish_term_breaks']}, "
              f"dropped {total['polish_dropped']}, dashes {total['dash_outputs']}/{total['polished']}, format {total['format_outputs']}/{total['polished']}")
        reasons = ", ".join(f"{k.replace('_', ' ')} {v}" for k, v in total["meaning_reasons"].items() if v)
        print(f"  polish: term error rate {percent(total['polished_term_error_rate'])}, "
              f"meaning changed {total['meaning_changed']}/{total['polished']} ({percent(total['meaning_rate'])}){f': {reasons}' if reasons else ''}")
    for label in ("decode", "stop_to_delivery", "polish"):
        if f"{label}_ms_median_short" in total or f"{label}_ms_median_longer" in total:
            parts = [f"{s} median {total.get(f'{label}_ms_median_{s}')} ms, p90 {total.get(f'{label}_ms_p90_{s}')} ms"
                     for s in ("short", "longer") if f"{label}_ms_median_{s}" in total]
            print(f"  {label.replace('_', ' ')}: " + "; ".join(parts))
    accepted = gold / "accepted.json"
    comparison = None
    base = against or (load(accepted)["run"] if accepted.exists() else None)
    if base and base != name:
        baseline = score(gold, load(gold / "runs" / f"{base}.json"))
        comparison = {"against": base}
        polished = lambda clips: {k: v for k, v in clips.items() if "polished_errors" in v}
        measures = [(label, num, den, baseline, scored) for label, num, den in COMPARED]
        measures += [(label, num, den, polished(baseline), polished(scored)) for label, num, den in COMPARED_POLISH]
        for label, num, den, left, right in measures:
            interval = personal.paired_bootstrap(left, right, num, den)
            if interval:
                comparison[label] = interval
                print(f"  vs {'' if against else 'accepted '}{base}: {label} {100 * interval['difference']:+.2f} points "
                      f"[{100 * interval['low']:+.2f}, {100 * interval['high']:+.2f}] over {interval['clips']} clips")
    save(gold / "scores" / f"{name}.json", {"summary": total, "comparison": comparison})
    return total


def cut_tails(clip, seconds, window):
    """The clip with a trailing hallucination segment removed: the last segment's
    words are exactly one of personal.TAILS and it starts within `window` seconds
    of the end, where whisper decodes a sub-second leftover of audio. Whisper's
    no-speech probability was near zero on every such tail, so it is not used."""
    segments = clip.get("segments")
    if not segments or len(segments) < 2:
        return clip
    last = segments[-1]
    if personal.words(last["text"]) not in personal.TAILS or (last.get("start") or 0) < seconds - window:
        return clip
    text = "".join(s["text"] for s in segments[:-1]).strip()
    return {**clip, "text": text, "segments": segments[:-1], "tail_cut": True}


def polish_plan(gold, configs, raw_run):
    """The gold clips with, for each, the mishearings it is given: the seeded
    pairs plus those the maintainer's corrections of the *other* half of the clips would
    teach (cross-fitted, so no clip is polished with a pair learned from its own
    correction). A proposed pair counts as accepted when it maps toward one of
    his terms, which is what accepting in History would do."""
    manifest = load(gold / "manifest.json")["clips"]
    run = load(gold / "runs" / f"{raw_run}.json")["clips"]
    refs = {r["recording_id"]: r["text"] for r in load(gold / "references.json")
            if r.get("gold") and not r.get("needs_check") and "[?]" not in r["text"]}
    corrections = {c["id"]: {"heard": run[c["id"]]["text"], "corrected": refs[c["id"]].replace("*", "")}
                   for c in manifest if c["id"] in refs and c["id"] in run}
    with tempfile.TemporaryDirectory() as temp:
        given, proposed = Path(temp) / "corrections.json", Path(temp) / "suggestions.json"
        save(given, corrections)
        subprocess.run(["cargo", "test", "-q", "-p", "metrix-writer-api", "--lib", "--",
                        "--ignored", "--exact", "vocabulary::learning::tests::gold_suggestions"],
                       cwd=ROOT, check=True, env={**os.environ, "WRITER_GOLD_CORRECTIONS": str(given),
                                                  "WRITER_GOLD_SUGGESTIONS": str(proposed)})
        suggestions = load(proposed)
    known = {tuple(t) for t in terms(gold)} | {tuple(personal.words(h)) for c in manifest for h in c.get("hints", [])}
    ids = sorted(suggestions)
    fold = {clip: i % 2 for i, clip in enumerate(ids)}
    learned = [[], []]
    for clip in ids:
        for s in suggestions[clip]:
            if s["kind"] == "mishearing" and tuple(personal.words(s["meant"])) in known:
                pair = {"heard": s["heard"], "meant": s["meant"]}
                if pair not in learned[fold[clip]]:
                    learned[fold[clip]].append(pair)
    seeded = configs.get("seeded", [])
    clips = []
    for c in manifest:
        row = run.get(c["id"]) or {}
        other = learned[1 - fold[c["id"]]] if c["id"] in fold else learned[0] + learned[1]
        clips.append({"id": c["id"], "job": c["job"], "s2d_ms": row.get("s2d_ms"), "polish_ms": row.get("polish_ms"),
                      "mishearings": seeded + [p for p in other if p["heard"] not in {s["heard"] for s in seeded}]})
    print(f"{len(clips)} clips; learned pairs {len(learned[0])} + {len(learned[1])} (two folds), seeded {len(seeded)}")
    return {"configs": configs["configs"], "clips": clips}


def add_runs(gold, decoded, by_job=False):
    manifest = load(gold / "manifest.json")["clips"]
    jobs = {c["job"]: c["id"] for c in manifest}
    names = []
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M")
    for config_name, body in decoded.items():
        clips = body["clips"] if "clips" in body else body
        if by_job:
            clips = {jobs[j]: v for j, v in clips.items() if j in jobs}
        name = f"{stamp}-{config_name}" if not by_job else config_name
        save(gold / "runs" / f"{name}.json", {"name": name, "config": body.get("config", {"name": config_name}), "created": stamp, "clips": clips})
        names.append(name)
    return names


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--gold")
    parser.add_argument("--host", default="windows")
    sub = parser.add_subparsers(dest="command", required=True)
    run = sub.add_parser("run")
    run.add_argument("--configs", required=True)
    sub.add_parser("accept").add_argument("run")
    scoring = sub.add_parser("score")
    scoring.add_argument("runs", nargs="+")
    scoring.add_argument("--against")
    derive = sub.add_parser("derive")
    derive.add_argument("run")
    derive.add_argument("--tails", type=float, required=True)
    refs = sub.add_parser("references")
    refs.add_argument("action", choices=["pull", "push"])
    refs.add_argument("file", nargs="?")
    planned = sub.add_parser("polish-plan")
    planned.add_argument("--configs", required=True)
    planned.add_argument("--raw", required=True, help="run whose text is polished (the delivered recognition)")
    replayed = sub.add_parser("polish-run")
    replayed.add_argument("--exe", required=True, help="metrix_writer_api test executable built for Windows")
    imported = sub.add_parser("import")
    imported.add_argument("file")
    imported.add_argument("--prefix", default="")
    args = parser.parse_args()
    gold = gold_dir(args.gold)

    if args.command == "run":
        with tempfile.TemporaryDirectory() as temp:
            copy_manifest = Path(temp) / "manifest.json"
            save(copy_manifest, load(gold / "manifest.json"))
            worker(args.host, "list", REMOTE)  # proves the profile is readable before a long replay
            windows(f'New-Item -ItemType Directory -Force "{REMOTE}" | Out-Null', args.host)
            copy_to(args.host, copy_manifest, "manifest.json")
            copy_to(args.host, args.configs, "configs.json")
            worker(args.host, "replay", REMOTE, f"{REMOTE}\\manifest.json", f"{REMOTE}\\configs.json")
            copy_from(args.host, "replay.json", Path(temp) / "replay.json")
            windows(f'Remove-Item -Force "{REMOTE}\\replay.json","{REMOTE}\\clips.json","{REMOTE}\\manifest.json"', args.host)
            names = add_runs(gold, load(Path(temp) / "replay.json"))
        if not (gold / "references.json").exists():
            print(f"saved {', '.join(names)}; no references yet, so nothing is scored")
            return
        for name in names:
            report(gold, name)
    elif args.command == "accept":
        load(gold / "runs" / f"{args.run}.json")
        save(gold / "accepted.json", {"run": args.run, "at": datetime.datetime.now().isoformat(timespec="seconds")})
        print(f"accepted {args.run}")
    elif args.command == "score":
        for name in args.runs:
            report(gold, name, args.against)
    elif args.command == "derive":
        run = load(gold / "runs" / f"{args.run}.json")
        lengths = {c["id"]: c["ms"] / 1000 for c in load(gold / "manifest.json")["clips"]}
        clips = {k: cut_tails(v, lengths.get(k, 0), args.tails) for k, v in run["clips"].items()}
        name = f"{args.run}-tails{args.tails:g}"
        save(gold / "runs" / f"{name}.json", {**run, "name": name, "clips": clips,
                                              "config": {**run["config"], "tails": args.tails}})
        print(f"{name}: {sum(c.get('tail_cut', False) for c in clips.values())} tails cut")
    elif args.command == "references":
        with tempfile.TemporaryDirectory() as temp:
            if args.action == "pull":
                worker(args.host, "references-pull", REMOTE)
                copy_from(args.host, "references.json", Path(temp) / "references.json")
                windows(f'Remove-Item -Force "{REMOTE}\\references.json"', args.host)
                pulled = load(Path(temp) / "references.json")
                save(gold / "references.json", pulled)
                print(f"{len(pulled)} references, {sum(r.get('needs_check', False) for r in pulled)} waiting for a check")
            else:
                copy_to(args.host, args.file, "drafts.json")
                print(worker(args.host, "references-push", REMOTE, f"{REMOTE}\\drafts.json").strip())
                windows(f'Remove-Item -Force "{REMOTE}\\drafts.json"', args.host)
    elif args.command == "polish-plan":
        save(gold / "polish-plan.json", polish_plan(gold, load(args.configs), args.raw))
    elif args.command == "polish-run":
        # Paid: every configuration polishes every planned gold clip once.
        windows(f'New-Item -ItemType Directory -Force "{REMOTE}" | Out-Null', args.host)
        copy_to(args.host, args.exe, "api-tests.exe")
        copy_to(args.host, gold / "polish-plan.json", "polish-plan.json")
        done = windows(f'$env:WRITER_REPLAY_ROOT="$env:LOCALAPPDATA\\com.tricktic.dictate"; '
                       f'$env:WRITER_GOLD_POLISH="{REMOTE}\\polish-plan.json"; $env:WRITER_REPLAY_OUTPUT="{REMOTE}\\polish.json"; '
                       f'Remove-Item -Force -ErrorAction SilentlyContinue "{REMOTE}\\polish.json"; '
                       f'& "{REMOTE}\\api-tests.exe" --ignored --exact cleanup::modes::replay::gold_polish --nocapture 2>&1 | Select-Object -Last 5', args.host)
        print(done.stdout[-1500:])
        with tempfile.TemporaryDirectory() as temp:
            copy_from(args.host, "polish.json", Path(temp) / "polish.json")
            windows(f'Remove-Item -Force "{REMOTE}\\polish.json","{REMOTE}\\polish-plan.json","{REMOTE}\\api-tests.exe"', args.host)
            stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M")
            decoded = {f"{stamp}-{name}": body for name, body in load(Path(temp) / "polish.json").items()}
        for name in add_runs(gold, decoded, by_job=True):
            report(gold, name)
    elif args.command == "import":
        decoded = load(args.file)
        if args.prefix:
            decoded = {f"{args.prefix}{k}": v for k, v in decoded.items()}
        for name in add_runs(gold, decoded, by_job=True):
            print(f"imported {name}")


if __name__ == "__main__":
    main()
