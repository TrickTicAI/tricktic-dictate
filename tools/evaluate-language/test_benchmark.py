import copy
import json
from pathlib import Path
import tempfile
import unittest
import wave

from benchmark import ROOT, archive, compare, digest, encoded, load_archive, report
from evaluate import score, exact_occurrences
from prepare import import_public, noise


class BenchmarkTests(unittest.TestCase):
    def setUp(self):
        self.fixtures = ROOT / "tests/fixtures/language/benchmark-v1.json"
        self.predictions = ROOT / "tests/fixtures/language/benchmark-wrong-v1.json"
        self.manifest = json.loads(self.fixtures.read_bytes())
        self.run = json.loads(self.predictions.read_bytes())

    def test_normalized_zero_does_not_hide_sign_or_punctuation_loss(self):
        result = score("Balance -12.50", "Balance 12.50")
        self.assertEqual(result["word_error_rate"], 0)
        self.assertEqual(result["critical_errors"][0]["kind"], "numeric_sign_or_value")
        result = score("status: pending", "status pending", [{"kind": "punctuation", "value": "status: pending"}])
        self.assertEqual(result["word_error_rate"], 0)
        self.assertTrue(result["critical_errors"])

    def test_exact_value_boundaries_and_negation_additions(self):
        for text in ("alex@example.com.evil", "malex@example.com", "alex@example.com0"):
            self.assertEqual(exact_occurrences(text, "alex@example.com"), 0)
        self.assertEqual(exact_occurrences("Email alex@example.com.", "alex@example.com"), 1)
        self.assertEqual(exact_occurrences("1200", "120"), 0)
        for expected, actual in (("Do not send", "Do send"), ("Do send", "Do not send"), ("לא לשלוח", "לשלוח"), ("אין למחוק", "למחוק")):
            self.assertTrue(any(e["kind"] == "negation" for e in score(expected, actual)["critical_errors"]))

    def test_wrong_corpus_detected_per_language_and_stage_without_measured_cells(self):
        result = report(self.manifest, self.run)
        for language in ("en", "he", "mixed"):
            rows = {r["id"]: r for r in result["results"] if r["input_language"] == language}
            for case in ("sign", "negation", "punctuation"):
                self.assertTrue(rows[f"{language}-{case}"]["raw"]["critical_errors"])
            snippet = rows[f"{language}-snippet"]
            self.assertEqual(snippet["raw"]["word_edits"], 0)
            self.assertFalse(snippet["cleanup"]["exact_values_preserved"])
        self.assertTrue(all(c["status"] == "unmeasured" for c in result["coverage"]))
        self.assertEqual(len(result["coverage"]), 18)

    def test_archives_retain_old_bytes_compare_and_reject_changed_fixtures(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            a = archive(self.fixtures, self.predictions, root / "runs")
            original = {p.name: p.read_bytes() for p in a.iterdir()}
            self.assertEqual(archive(self.fixtures, self.predictions, root / "runs"), a)
            better = copy.deepcopy(self.run)
            samples = {s["id"]: s for s in self.manifest["samples"]}
            for row in better["results"]:
                s = samples[row["id"]]
                row["raw_text"] = s["expected_text"]
                row["cleaned_text"] = s.get("processed_expected_text", s["expected_text"])
            better["stages"]["raw"]["prompt"] = "preserve signs"
            path = root / "better.json"
            path.write_bytes(encoded(better))
            b = archive(self.fixtures, path, root / "runs")
            result = compare(a, b)
            self.assertNotEqual(a, b)
            self.assertTrue(any(p["critical_errors_before"] and not p["critical_errors_after"] for p in result["pairs"]))
            self.assertTrue(any(p["reason"].startswith("different raw") for p in result["excluded"]))
            self.assertEqual({p.name: p.read_bytes() for p in a.iterdir()}, original)
            changed = copy.deepcopy(self.manifest)
            changed["fixture_version"] += "-changed"
            better["fixture_version"] = changed["fixture_version"]
            fixture_path = root / "fixtures.json"
            fixture_path.write_bytes(encoded(changed)); path.write_bytes(encoded(better))
            c = archive(fixture_path, path, root / "runs")
            with self.assertRaisesRegex(ValueError, "identical fixtures"):
                compare(a, c)
            (b / "report.json").chmod(0o644)
            (b / "report.json").write_bytes(b"{}")
            with self.assertRaisesRegex(ValueError, "integrity"):
                load_archive(b)

    def test_real_audio_verification_metadata_and_unmeasured_cells(self):
        with tempfile.TemporaryDirectory() as directory:
            audio = Path(directory) / "clip.wav"
            self.write_wav(audio)
            run = copy.deepcopy(self.run)
            run["kind"] = "audio_run"
            run["results"] = [run["results"][0]]
            row = run["results"][0]
            row.update(consent_confirmed=True, audio_path=str(audio), audio_sha256=digest(audio.read_bytes()), codec="pcm_s16le/16000/mono", recording_duration_ms=1000, latency_ms={"raw": 12, "cleanup": 15})
            result = report(self.manifest, run)
            measured = [c for c in result["coverage"] if c["status"] == "measured"]
            self.assertEqual(len(measured), 2)
            self.assertTrue(all(c["language"] == "en" and c["device"] == "windows_gpu" for c in measured))
            row["latency_ms"]["raw"] = float("nan")
            with self.assertRaisesRegex(ValueError, "latency"):
                report(self.manifest, run)
            row["latency_ms"]["raw"] = 12
            row["audio_sha256"] = "0" * 64
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                report(self.manifest, run)
            row["audio_sha256"] = digest(audio.read_bytes())
            del run["stages"]["raw"]["prompt"]
            with self.assertRaisesRegex(ValueError, "literal prompt"):
                report(self.manifest, run)

    def test_public_import_and_repeatable_noise_keep_hashes_and_refuse_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            audio = root / "speech.wav"
            self.write_wav(audio)
            public = root / "public.json"
            public.write_bytes(encoded({"public_corpus_reuse_confirmed": True, "source": "test public manifest", "samples": [{"id": "public-test", "input_language": "en", "expected_text": "Hello", "audio_path": str(audio)}]}))
            imported = import_public(self.fixtures, public, "librispeech")
            self.assertEqual(imported["samples"][-1]["split"], "public")
            self.assertEqual(imported["samples"][-1]["audio_sha256"], digest(audio.read_bytes()))
            self.assertNotEqual(imported["fixture_version"], self.manifest["fixture_version"])
            a, b = root / "a.wav", root / "b.wav"
            receipt = noise(audio, a)
            noise(audio, b)
            self.assertEqual(a.read_bytes(), b.read_bytes())
            self.assertEqual(receipt["source_audio_sha256"], digest(audio.read_bytes()))
            with self.assertRaises(FileExistsError):
                noise(audio, a)

    def test_audio_variants_pair_only_with_verified_same_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, variant = root / "clean.wav", root / "noise.wav"
            self.write_wav(source)
            preparation = noise(source, variant)
            run = copy.deepcopy(self.run)
            run["kind"] = "audio_run"
            run["results"] = [run["results"][0]]
            row = run["results"][0]
            row.pop("cleaned_text")
            row.update(consent_confirmed=True, audio_path=str(source), audio_sha256=digest(source.read_bytes()), codec="pcm_s16le/16000/mono", recording_duration_ms=1000, latency_ms={"raw": 12})
            path = root / "run.json"
            path.write_bytes(encoded(run))
            a = archive(self.fixtures, path, root / "runs")
            row.update(preparation)
            path.write_bytes(encoded(run))
            b = archive(self.fixtures, path, root / "runs")
            pairs = compare(a, b)["pairs"]
            self.assertEqual(len(pairs), 1)
            self.assertNotEqual(pairs[0]["audio_sha256_before"], pairs[0]["audio_sha256_after"])
            row["source_audio_sha256"] = "0" * 64
            with self.assertRaisesRegex(ValueError, "source hash"):
                report(self.manifest, run)

    def test_native_runner_refuses_server_cpu_execution(self):
        import importlib.util
        import os
        if os.name == "nt":
            self.skipTest("server guard is tested on a non-Windows host")
        path = ROOT / "scripts/bench/language-quality-windows.py"
        spec = importlib.util.spec_from_file_location("native_benchmark", path)
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        with self.assertRaisesRegex(ValueError, "server CPU inference is forbidden"):
            runner.execute(None)

    @staticmethod
    def write_wav(path):
        with wave.open(str(path), "wb") as wav:
            wav.setnchannels(1); wav.setsampwidth(2); wav.setframerate(16000)
            wav.writeframes(b"\xe8\x03\x18\xfc" * 8000)


if __name__ == "__main__":
    unittest.main()
