#!/usr/bin/env python3
"""Score supplied transcripts; never records audio or invokes an inference engine."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import unicodedata

ROOT = Path(__file__).resolve().parents[2]


def normalized(text):
    # Retain letters, marks and numbers in both scripts; punctuation/case is formatting.
    return " ".join("".join(c if unicodedata.category(c)[0] in "LMN" else " "
                            for c in unicodedata.normalize("NFKC", text).casefold()).split())


def alignment(expected, actual):
    """Levenshtein alignment with deterministic deletion/substitution/insertion counts."""
    if expected == actual:
        return 0, []
    prefix = 0
    while prefix < min(len(expected), len(actual)) and expected[prefix] == actual[prefix]:
        prefix += 1
    expected, actual = expected[prefix:], actual[prefix:]
    suffix = 0
    while suffix < min(len(expected), len(actual)) and expected[-suffix-1] == actual[-suffix-1]:
        suffix += 1
    if suffix:
        expected, actual = expected[:-suffix], actual[:-suffix]
    table = [[0] * (len(actual) + 1) for _ in range(len(expected) + 1)]
    for i in range(len(expected) + 1):
        table[i][0] = i
    for j in range(len(actual) + 1):
        table[0][j] = j
    for i, left in enumerate(expected, 1):
        for j, right in enumerate(actual, 1):
            table[i][j] = min(table[i-1][j] + 1, table[i][j-1] + 1,
                              table[i-1][j-1] + (left != right))
    i, j = len(expected), len(actual)
    changes = []
    while i or j:
        if i and j and expected[i-1] == actual[j-1] and table[i][j] == table[i-1][j-1]:
            i, j = i-1, j-1
        elif i and table[i][j] == table[i-1][j] + 1:
            changes.append({"operation": "omission", "reference_index": i-1, "expected": expected[i-1]})
            i -= 1
        elif j and table[i][j] == table[i][j-1] + 1:
            changes.append({"operation": "addition", "result_index": j-1, "actual": actual[j-1]})
            j -= 1
        else:
            changes.append({"operation": "substitution", "reference_index": i-1, "expected": expected[i-1], "actual": actual[j-1]})
            i, j = i-1, j-1
    for change in changes:
        for index in ("reference_index", "result_index"):
            if index in change:
                change[index] += prefix
    return table[-1][-1], list(reversed(changes))


def distance(expected, actual):
    # Linear space for potentially long dictation character metrics.
    prefix = 0
    while prefix < min(len(expected), len(actual)) and expected[prefix] == actual[prefix]:
        prefix += 1
    expected, actual = expected[prefix:], actual[prefix:]
    suffix = 0
    while suffix < min(len(expected), len(actual)) and expected[-suffix-1] == actual[-suffix-1]:
        suffix += 1
    if suffix:
        expected, actual = expected[:-suffix], actual[:-suffix]
    if not expected or not actual:
        return max(len(expected), len(actual))
    row = list(range(len(actual) + 1))
    for i, left in enumerate(expected, 1):
        next_row = [i]
        for j, right in enumerate(actual, 1):
            next_row.append(min(next_row[-1]+1, row[j]+1, row[j-1]+(left != right)))
        row = next_row
    return row[-1]


def exact_occurrences(text, value):
    """Whole values only: an email/domain/number embedded in a larger value fails."""
    return len(re.findall(r"(?<![\w@.+/%-])" + re.escape(value) + r"(?![\w@+/%-]|\.\w)", text))


def critical_errors(expected, actual, checks=()):
    # Keep signs, decimal/group separators and percent marks. WER intentionally
    # discards these. This is a lexical safeguard, not semantic equivalence.
    number = r"(?<!\w)[+\-−]?\d+(?:[.,:/-]\d+)*(?:\s?%)?"
    negation = r"\b(?:no|not|never|cannot|can't|don't|doesn't|didn't|won't|isn't|without|לא|אל|אין|אינו|אינה|בלי|ללא)\b"
    errors = []
    for name, pattern in (("numeric_sign_or_value", number), ("negation", negation)):
        before = Counter(re.findall(pattern, expected.casefold()))
        after = Counter(re.findall(pattern, actual.casefold()))
        if before != after:
            errors.append({"kind": name, "expected": dict(before), "actual": dict(after)})
    for check in checks:
        value = check["value"]
        before, after = exact_occurrences(expected, value), exact_occurrences(actual, value)
        if before == 0:
            raise ValueError(f"critical value absent from reference: {value}")
        if before != after:
            errors.append({"kind": check["kind"], "value": value,
                           "expected_count": before, "actual_count": after})
    return errors


def score(expected, actual, checks=()):
    reference, result = normalized(expected), normalized(actual)
    words = reference.split()
    edits, changes = alignment(words, result.split())
    character_edits = distance(reference, result)
    scripts = {"hebrew": r"[\u0590-\u05ff]", "latin": r"[a-zA-Z]"}
    return {
        "word_error_rate": edits / len(words) if words else None,
        "word_edits": edits,
        "omitted_tokens": [change for change in changes if change["operation"] == "omission"],
        "added_tokens": [change for change in changes if change["operation"] == "addition"],
        "substitutions": [change for change in changes if change["operation"] == "substitution"],
        "normalized_character_edits": character_edits,
        "normalized_character_error_rate": character_edits / len(reference) if reference else None,
        "exact_character_edits_including_formatting": distance(expected, actual),
        "formatting_only": expected != actual and reference == result,
        "missing_script_flags": [name for name, pattern in scripts.items() if re.search(pattern, expected) and not re.search(pattern, actual)],
        "critical_errors": critical_errors(expected, actual, checks),
    }


def evaluate(manifest, run):
    kind = run["kind"]
    if kind not in ("synthetic_transcripts", "audio_run"):
        raise ValueError("kind must be synthetic_transcripts or audio_run")
    for key in ("model", "model_revision", "run_at", "settings"):
        if not run.get(key):
            raise ValueError(f"run metadata requires {key}")
    samples = {sample["id"]: sample for sample in manifest["samples"]}
    reports, seen = [], set()
    for result in run["results"]:
        sample_id = result["id"]
        if sample_id in seen or sample_id not in samples:
            raise ValueError(f"duplicate or unknown sample: {sample_id}")
        seen.add(sample_id)
        sample = samples[sample_id]
        if kind == "audio_run":
            if not (result.get("consent_confirmed") is True or result.get("public_corpus_reuse_confirmed") is True) or not re.fullmatch(r"[a-f0-9]{64}", result.get("audio_sha256", "")):
                raise ValueError(f"{sample_id}: real audio requires consent_confirmed and audio_sha256")
            if sample.get("requires_private_reference"):
                raise ValueError(f"{sample_id}: replace placeholder with complete private reference and clear requires_private_reference")
        if result.get("input_language") != sample["input_language"]:
            raise ValueError(f"{sample_id}: run input_language must match fixture")
        latency = None
        if "stopped_at_ms" in result and "result_at_ms" in result:
            latency = result["result_at_ms"] - result["stopped_at_ms"]
            if latency < 0:
                raise ValueError("result timestamp precedes stop")
        review = result.get("review", {})
        translation = review.get("unwanted_translation_events")
        if translation is not None and (type(translation) is not int or translation < 0):
            raise ValueError("unwanted_translation_events must be a nonnegative integer or null")
        report = {
            "id": sample_id, "input_language": sample["input_language"],
            "raw": score(sample["expected_text"], result["raw_text"], sample.get("raw_critical_checks", [])),
            "stop_to_result_ms": latency, "recording_duration_ms": result.get("recording_duration_ms"),
            "human_review": {"unwanted_translation_events": translation,
                             "correction_edits": review.get("correction_edits"),
                             "correction_seconds": review.get("correction_seconds"),
                             "omitted_spans": review.get("omitted_spans"),
                             "repeated_spans": review.get("repeated_spans")},
            "mixed_translation_gate": "not_applicable" if sample["input_language"] != "mixed" else
                "unreviewed" if translation is None else "fail" if translation else "pass",
        }
        if "cleaned_text" in result:
            report["cleanup"] = score(sample.get("processed_expected_text", sample["expected_text"]), result["cleaned_text"], sample.get("cleanup_critical_checks", sample.get("raw_critical_checks", [])))
            report["cleanup"]["exact_values_preserved"] = all(exact_occurrences(result["cleaned_text"], value) == exact_occurrences(sample.get("processed_expected_text", sample["expected_text"]), value) > 0 for value in sample["exact_values"]) if sample.get("exact_values") else None
        reports.append(report)
    return {
        "kind": kind,
        "evidence": "Synthetic transcript regression only; no speech accuracy or measured inference latency." if kind == "synthetic_transcripts" else "Supplied audio-run records; inspect consent, exact settings and human review. Harness does not verify inference provenance.",
        "metadata": {key: run[key] for key in ("model", "model_revision", "run_at", "settings")},
        "missing_samples": sorted(set(samples) - seen), "results": reports,
        "notes": ["Omissions and additions are token alignment candidates, not semantic judgments.",
                  "Missing-script flags cannot prove translation. Unwanted translation requires human review.",
                  "Character edit distances estimate correction work; measured human edits/time remain separate.",
                  "Formatting normalization ignores punctuation/case; lexical numeric/negation and annotated critical values are checked separately.",
                  "Time saved remains an estimate. No external-app corrections are observed or collected."],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    parser.add_argument("--fixtures", type=Path, default=ROOT / "tests/fixtures/language/samples.json")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = evaluate(json.loads(args.fixtures.read_text()), json.loads(args.run.read_text()))
    report["manifest_sha256"] = hashlib.sha256(args.fixtures.read_bytes()).hexdigest()
    report["run_sha256"] = hashlib.sha256(args.run.read_bytes()).hexdigest()
    rendered = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.write_text(rendered)
    else:
        print(rendered, end="")


if __name__ == "__main__":
    main()
