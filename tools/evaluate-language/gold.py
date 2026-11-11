#!/usr/bin/env python3
"""Build the maintainer's private gold set (MW-121).

  select   choose about 150 clips stratified by length and content from the
           clip list the Windows worker exports (scorecard-worker.py list)
  packets  write blind drafting packets: each clip's distinct recognitions in
           shuffled order, for independent agent drafters
  merge    compare two independent drafts per clip; agreeing drafts become
           references, disagreeing ones are queued for the maintainer to correct

Everything reads and writes the private gold folder; nothing here prints
transcript text, and no file it writes belongs in the repository.
"""
import argparse
import json
import random
from pathlib import Path

import personal

# Length strata from the dictation review; ~150 clips with the long tail kept.
TARGETS = {"short": 60, "medium": 55, "long": 35}


def load(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def save(path, value):
    Path(path).parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=1), encoding="utf-8")
    Path(path).chmod(0o600)


def stratum(ms):
    return "short" if ms <= 15_000 else "medium" if ms <= 60_000 else "long"


def content(clip, terms):
    """Coarse content tags; the term tag uses both what was heard and the polish."""
    tokens = personal.words(f"{clip.get('raw') or ''} {clip.get('polished') or ''}")
    tags = []
    if any(personal.occurrences(tokens, t) for t in terms):
        tags.append("project_terms")
    if not personal.words(clip.get("raw") or ""):
        tags.append("empty_result")
    if any("֐" <= c <= "׿" for c in clip.get("raw") or ""):
        tags.append("hebrew")
    return tags or ["other"]


def select(clips, keep, targets=TARGETS, seed=121):
    """Keeps the already-sampled clips, then fills each length stratum, half
    from clips with project terms and half from the rest, plus a few empty
    results (lost speech and silence hallucinations)."""
    terms = [personal.words(t) for t in personal.PROJECT_TERMS]
    generator = random.Random(seed)
    # Only clips whose audio is still saved can be replayed.
    by_id = {c["rec"]: c for c in clips if c.get("ms") and c.get("audio", True)}
    chosen = [by_id[r] for r in keep if r in by_id]
    taken = {c["rec"] for c in chosen}
    for name, target in targets.items():
        have = sum(stratum(c["ms"]) == name for c in chosen)
        pool = [c for c in by_id.values() if c["rec"] not in taken and stratum(c["ms"]) == name]
        generator.shuffle(pool)
        groups = {
            "empty": [c for c in pool if "empty_result" in content(c, terms) and c["ms"] >= 2_000][:3 if name == "short" else 0],
            "terms": [c for c in pool if "project_terms" in content(c, terms)],
            "other": [c for c in pool if content(c, terms) == ["other"]],
        }
        need = max(0, target - have)
        picks = groups["empty"][:need]
        rest = need - len(picks)
        half = (rest + 1) // 2
        picks += groups["terms"][:half]
        picks += [c for c in groups["other"] if c not in picks][:rest - min(half, len(groups["terms"]))]
        # When one content group runs short, the stratum is filled from the rest.
        picks += [c for c in pool if c not in picks][:need - len(picks)]
        for clip in picks:
            chosen.append(clip)
            taken.add(clip["rec"])
    return {"version": 1, "clips": [
        {"id": c["rec"], "job": c["job"], "ms": c["ms"], "stratum": stratum(c["ms"]),
         "hints": c.get("hints", []), "tags": content(c, terms)}
        for c in sorted(chosen, key=lambda c: c["t"])
    ]}


def packets(manifest, runs, polished, size, seed=11):
    """Blind packets: distinct candidate texts per clip, shuffled, labelled A, B, ..."""
    generator = random.Random(seed)
    out, current = [], []
    for index, clip in enumerate(manifest["clips"], 1):
        texts = []
        for run in runs:
            text = ((run["clips"].get(clip["id"]) or {}).get("text") or "").strip()
            if text not in texts:
                texts.append(text)
        generator.shuffle(texts)
        lines = [f"### clip {clip['id']} ({clip['ms'] / 1000:.1f} s)"]
        lines += [f"{'ABCDEFGHIJ'[i]}: {t}" for i, t in enumerate(texts)]
        context = (polished.get(clip["id"]) or "").strip()
        if context:
            lines.append(f"POLISHED (context only, may contain invented edits): {context}")
        current.append("\n".join(lines) + "\n")
        if len(current) == size or index == len(manifest["clips"]):
            out.append("\n".join(current))
            current = []
    return out


def merge(manifest, first, second, now_ms):
    """Agreeing drafts (same words, nothing unresolved) are accepted; the rest wait for the maintainer."""
    references = []
    for clip in manifest["clips"]:
        a, b = first.get(clip["id"]), second.get(clip["id"])
        if a is None and b is None:
            continue
        a = a if a is not None else b
        b = b if b is not None else a
        agree = personal.reference_words(a) is not None and personal.reference_words(a) == personal.reference_words(b)
        references.append({
            "recording_id": clip["id"], "job_id": clip["job"],
            "text": a.replace("*", ""), "gold": True, "source": "draft", "needs_check": not agree,
            **({} if agree else {"alternative": b.replace("*", "")}),
            "updated_at_ms": now_ms,
        })
    return references


def main():
    import time
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--gold", required=True)
    sub = parser.add_subparsers(dest="command", required=True)
    chosen = sub.add_parser("select")
    chosen.add_argument("--clips", required=True)
    chosen.add_argument("--keep", help="JSON list of recording ids already sampled")
    chosen.add_argument("--model", help="only clips recognized by this model (today's pipeline)")
    chosen.add_argument("--language", help="only clips with this input language")
    blind = sub.add_parser("packets")
    blind.add_argument("--runs", nargs="+", required=True)
    blind.add_argument("--size", type=int, default=50)
    blind.add_argument("--seed", type=int, default=11, help="a different order for each independent drafter")
    blind.add_argument("--name", default="packet")
    joined = sub.add_parser("merge")
    joined.add_argument("first")
    joined.add_argument("second")
    args = parser.parse_args()
    gold = Path(args.gold)

    if args.command == "select":
        clips = [c for c in load(args.clips)
                 if (not args.model or c.get("model") == args.model) and (not args.language or c.get("lang") == args.language)]
        manifest = select(clips, load(args.keep) if args.keep else [])
        save(gold / "manifest.json", manifest)
        counts = {}
        for clip in manifest["clips"]:
            counts[clip["stratum"]] = counts.get(clip["stratum"], 0) + 1
        print(f"{len(manifest['clips'])} clips: {counts}")
    elif args.command == "packets":
        manifest = load(gold / "manifest.json")
        runs = [load(gold / "runs" / f"{name}.json") for name in args.runs]
        polished = {k: v.get("polished") for run in runs for k, v in run["clips"].items() if v.get("polished")}
        for index, text in enumerate(packets(manifest, runs, polished, args.size, args.seed), 1):
            path = gold / "drafting" / f"{args.name}-{index}.txt"
            path.parent.mkdir(exist_ok=True, mode=0o700)
            path.write_text(text, encoding="utf-8")
            path.chmod(0o600)
            print(path.name)
    elif args.command == "merge":
        manifest = load(gold / "manifest.json")
        references = merge(manifest, load(args.first), load(args.second), int(time.time() * 1000))
        save(gold / "drafts.json", references)
        print(f"{len(references)} drafted, {sum(r['needs_check'] for r in references)} queued for a check")


if __name__ == "__main__":
    main()
