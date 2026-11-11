#!/usr/bin/env python3
"""Personal scorecard metrics (MW-121): scores recognitions and polished text
of the maintainer's own clips against corrected references.

Never records audio, invokes a model or prints transcript text; callers pass
texts in and get counts out. References and runs stay in private storage.
"""
import random
import re
import statistics
import unicodedata

from evaluate import alignment

# The words his agent prompts depend on (docs/features/dictation-accuracy-plan.md).
# A private terms.json in the gold folder replaces this list.
PROJECT_TERMS = [
    "merge", "merged", "merging", "worktree", "worktrees", "card", "cards", "Claude", "Claude Code",
    "Codex", "agent work", "waiting to merge", "orchestration board", "repo", "PR", "PRs", "CI",
    "OAuth", "standalone", "queue",
]
# Whisper's habitual inventions on silence and tails.
TAILS = [["thank", "you"], ["thanks", "for", "watching"], ["thank", "you", "for", "watching"], ["you"], ["bye"]]
DASHES = re.compile(r"\S\s*[—–]\s*\S")
BULLET = re.compile(r"^\s*(?:[-*•]|\d+[.)])\s+", re.M)
DROPPED_SHARE = 0.2


def words(text):
    """Case, punctuation and hyphens are formatting; apostrophes stay inside words."""
    text = unicodedata.normalize("NFKC", text or "").casefold().replace("’", "'").replace("‘", "'")
    return "".join(c if c == "'" or unicodedata.category(c)[0] in "LMN" else " " for c in text).split()


def reference_words(reference):
    """None when the reference still has an unresolved span marked [?]."""
    if "[?]" in reference:
        return None
    return words(reference.replace("*", ""))


def occurrences(tokens, term):
    n = len(term)
    return sum(tokens[i:i + n] == term for i in range(len(tokens) - n + 1)) if n else 0


def term_hits(reference, hypothesis, terms):
    """{term index: (occurrences in the reference, of them present in the hypothesis)}."""
    hits = {}
    for index, term in enumerate(terms):
        wanted = occurrences(reference, term)
        if wanted:
            hits[index] = (wanted, min(wanted, occurrences(hypothesis, term)))
    return hits


def loops(tokens):
    """Longest run of one 2-6 word phrase repeated back to back three or more times."""
    for n in range(2, 7):
        for i in range(len(tokens) - 3 * n + 1):
            phrase = tokens[i:i + n]
            if tokens[i + n:i + 2 * n] == phrase and tokens[i + 2 * n:i + 3 * n] == phrase:
                return True
    return False


def hallucinated(reference, hypothesis):
    if not reference:
        return bool(hypothesis)
    for tail in TAILS:
        if hypothesis[-len(tail):] == tail and reference[-len(tail):] != tail:
            return True
    return loops(hypothesis) and not loops(reference)


def omissions(reference, hypothesis):
    _, changes = alignment(reference, hypothesis)
    return sum(c["operation"] == "omission" for c in changes)


def format_violations(text):
    """Dashes are counted separately; the rest are the style breaks the review found."""
    reasons = []
    if "‑" in text:
        reasons.append("non_breaking_hyphen")
    if "'" in text and "’" in text:
        reasons.append("mixed_apostrophes")
    if BULLET.search(text):
        reasons.append("bullets")
    if "\n\n" in text.strip() and len(words(text)) < 60:
        reasons.append("short_split_into_paragraphs")
    return reasons


def score_clip(reference, recognized, terms, polished=None):
    """Counts for one clip. `reference` is a word list from reference_words."""
    hypothesis = words(recognized)
    errors, _ = alignment(reference, hypothesis)
    heard = term_hits(reference, hypothesis, terms)
    total, hit = sum(w for w, _ in heard.values()), sum(h for _, h in heard.values())
    clip = {
        "words": len(reference), "errors": errors, "terms": total, "term_misses": total - hit,
        "hallucination": hallucinated(reference, hypothesis),
        "dropped": len(reference) >= 5 and omissions(reference, hypothesis) >= DROPPED_SHARE * len(reference),
    }
    if polished is not None:
        import meaning  # noqa: PLC0415 (meaning imports this module)
        shaped = words(polished)
        kept = term_hits(reference, shaped, terms)
        changed = meaning.changes(recognized, polished)
        clip.update({
            "polished_term_misses": total - sum(h for _, h in kept.values()),
            # What the polish was given against what it returned (MW-123).
            "meaning": changed, "meaning_changed": int(bool(changed)), "one": 1,
            "polished_errors": alignment(reference, shaped)[0],
            # Per term: occurrences the polish repaired, and ones heard right that it broke.
            "polish_fixes": sum(max(0, kept[t][1] - h) for t, (_, h) in heard.items()),
            "polish_breaks": sum(max(0, h - kept[t][1]) for t, (_, h) in heard.items()),
            "polish_dropped": len(reference) >= 5 and omissions(reference, shaped) >= DROPPED_SHARE * len(reference),
            "dashes": bool(DASHES.search(polished)),
            "format": format_violations(polished),
        })
    return clip


def percentile(values, share):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(share * len(ordered)))] if ordered else None


def ratio(clips, numerator, denominator):
    below = sum(c[denominator] for c in clips)
    return sum(c[numerator] for c in clips) / below if below else None


def summary(clips):
    """Aggregate a run's scored clips: {clip_id: score_clip(...) plus ms, stratum}."""
    rows = list(clips.values())
    out = {
        "clips": len(rows), "words": sum(r["words"] for r in rows),
        "wer": ratio(rows, "errors", "words"),
        "term_error_rate": ratio(rows, "term_misses", "terms"), "term_occurrences": sum(r["terms"] for r in rows),
        "hallucinations": sum(r["hallucination"] for r in rows), "dropped": sum(r["dropped"] for r in rows),
    }
    for stratum in ("short", "medium", "long"):
        part = [r for r in rows if r.get("stratum") == stratum]
        out[f"wer_{stratum}"] = ratio(part, "errors", "words")
    polished = [r for r in rows if "polished_errors" in r]
    if polished:
        out.update({
            "polished": len(polished), "polished_wer": ratio(polished, "polished_errors", "words"),
            "polish_term_fixes": sum(r["polish_fixes"] for r in polished),
            "polish_term_breaks": sum(r["polish_breaks"] for r in polished),
            "polish_dropped": sum(r["polish_dropped"] for r in polished),
            "dash_outputs": sum(r["dashes"] for r in polished),
            "format_outputs": sum(bool(r["format"]) for r in polished),
            "polished_term_error_rate": ratio(polished, "polished_term_misses", "terms"),
            "meaning_changed": sum(r["meaning_changed"] for r in polished),
            "meaning_rate": ratio(polished, "meaning_changed", "one"),
            "meaning_reasons": {reason: sum(reason in r["meaning"] for r in polished)
                                for reason in ("dropped_sentence", "dropped_content", "negation", "person", "question")},
        })
    for key, label in (("ms", "decode"), ("s2d_ms", "stop_to_delivery"), ("polish_ms", "polish")):
        for stratum, test in (("short", lambda r: r.get("stratum") == "short"), ("longer", lambda r: r.get("stratum") != "short")):
            values = [r[key] for r in rows if test(r) and isinstance(r.get(key), (int, float))]
            if values:
                out[f"{label}_ms_median_{stratum}"] = statistics.median(values)
                out[f"{label}_ms_p90_{stratum}"] = percentile(values, 0.9)
    return out


def paired_bootstrap(left, right, numerator="errors", denominator="words", rounds=2000, seed=0):
    """Difference right - left of a ratio over shared clips, with a 95% interval
    from resampling clips (the review's method)."""
    shared = sorted(set(left) & set(right))
    if not shared:
        return None
    def value(clips, ids):
        below = sum(clips[i][denominator] for i in ids)
        return sum(clips[i][numerator] for i in ids) / below if below else 0.0
    point = value(right, shared) - value(left, shared)
    generator = random.Random(seed)
    draws = []
    for _ in range(rounds):
        sample = [generator.choice(shared) for _ in shared]
        draws.append(value(right, sample) - value(left, sample))
    draws.sort()
    return {"clips": len(shared), "difference": point,
            "low": draws[int(0.025 * rounds)], "high": draws[int(0.975 * rounds) - 1]}
