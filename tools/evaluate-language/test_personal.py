#!/usr/bin/env python3
import unittest

import gold
import personal

TERMS = [personal.words(t) for t in personal.PROJECT_TERMS]


class MetricTests(unittest.TestCase):
    def test_words_keep_apostrophes_and_split_hyphens(self):
        self.assertEqual(personal.words("It’s the RS-460 merge, OK?"), ["it's", "the", "rs", "460", "merge", "ok"])
        self.assertEqual(personal.words("שלום, עולם"), ["שלום", "עולם"])
        self.assertIsNone(personal.reference_words("what [?] was said"))
        self.assertEqual(personal.reference_words("so this card* is done"), ["so", "this", "card", "is", "done"])

    def test_project_term_misses_and_polish_fixes_and_breaks(self):
        reference = personal.words("merge the card and clean up the worktrees")
        clip = personal.score_clip(reference, "marriage the card and clean up the walktrees", TERMS,
                                   polished="Merge the call and clean up the walktrees.")
        self.assertEqual((clip["terms"], clip["term_misses"]), (3, 2))
        self.assertEqual(clip["errors"], 2)
        self.assertEqual((clip["polish_fixes"], clip["polish_breaks"]), (1, 1))

    def test_hallucinated_tails_loops_and_silence(self):
        said = personal.words("is the card waiting to merge")
        self.assertTrue(personal.hallucinated(said, personal.words("is the card waiting to merge. Thank you.")))
        self.assertFalse(personal.hallucinated(personal.words("okay thank you"), personal.words("Okay, thank you.")))
        self.assertTrue(personal.hallucinated([], personal.words("Thank you for watching")))
        self.assertTrue(personal.hallucinated(said, personal.words("is the card waiting it works it works it works")))

    def test_dropped_content_and_format(self):
        reference = personal.words("first do the review then merge the cards and after that deploy the newest version")
        self.assertTrue(personal.score_clip(reference, "first do the review", TERMS)["dropped"])
        clip = personal.score_clip(reference, "first do the review then merge the cards and after that deploy the newest version", TERMS,
                                   polished="First do the review — then merge.\n\n- deploy the new‑est version")
        self.assertTrue(clip["polish_dropped"])
        self.assertTrue(clip["dashes"])
        self.assertEqual(clip["format"], ["non_breaking_hyphen", "bullets", "short_split_into_paragraphs"])
        self.assertFalse(personal.score_clip(reference, "x", TERMS, polished="CRM-236 is done, it's merged")["dashes"])

    def test_summary_and_paired_bootstrap(self):
        left = {str(i): {"words": 10, "errors": 2, "terms": 1, "term_misses": 1, "hallucination": False, "dropped": False,
                         "stratum": "short", "ms": 100 + i} for i in range(20)}
        right = {k: dict(v, errors=1, term_misses=0) for k, v in left.items()}
        total = personal.summary(right)
        self.assertAlmostEqual(total["wer"], 0.1)
        self.assertEqual(total["term_error_rate"], 0)
        self.assertEqual(total["decode_ms_median_short"], 109.5)
        interval = personal.paired_bootstrap(left, right)
        self.assertAlmostEqual(interval["difference"], -0.1)
        self.assertLessEqual(interval["low"], interval["difference"])
        self.assertGreaterEqual(interval["high"], interval["difference"])
        self.assertIsNone(personal.paired_bootstrap(left, {}))


def clip(rec, ms, raw, t):
    return {"rec": rec, "job": f"job-{rec}", "ms": ms, "raw": raw, "t": t, "hints": []}


class GoldTests(unittest.TestCase):
    def test_select_keeps_the_sample_and_fills_each_length_stratum(self):
        clips = [clip(f"s{i}", 5_000, "merge the card" if i % 2 else "hello there", i) for i in range(40)]
        clips += [clip(f"m{i}", 30_000, "words", 100 + i) for i in range(30)]
        clips += [clip(f"l{i}", 90_000, "the worktrees", 200 + i) for i in range(10)]
        clips += [clip("e", 4_000, "", 300)]
        manifest = gold.select(clips, keep=["m0", "m1"], targets={"short": 10, "medium": 5, "long": 3})
        ids = [c["id"] for c in manifest["clips"]]
        self.assertIn("m0", ids)
        self.assertIn("e", ids)
        counts = {s: sum(c["stratum"] == s for c in manifest["clips"]) for s in ("short", "medium", "long")}
        self.assertEqual(counts, {"short": 10, "medium": 5, "long": 3})
        self.assertGreaterEqual(sum("project_terms" in c["tags"] for c in manifest["clips"] if c["stratum"] == "short"), 4)
        self.assertEqual(manifest, gold.select(clips, keep=["m0", "m1"], targets={"short": 10, "medium": 5, "long": 3}))

    def test_disagreeing_drafts_are_queued_and_agreeing_ones_accepted(self):
        manifest = {"clips": [{"id": "a", "job": "ja"}, {"id": "b", "job": "jb"}, {"id": "c", "job": "jc"}, {"id": "d", "job": "jd"}]}
        first = {"a": "Merge the card.", "b": "merge the card", "c": "what [?] said", "d": "card* done"}
        second = {"a": "merge the card", "b": "marry the card", "c": "what was said", "d": "card done"}
        out = {r["recording_id"]: r for r in gold.merge(manifest, first, second, 5)}
        self.assertFalse(out["a"]["needs_check"])
        self.assertNotIn("alternative", out["a"])
        self.assertTrue(out["b"]["needs_check"])
        self.assertEqual(out["b"]["alternative"], "marry the card")
        self.assertTrue(out["c"]["needs_check"])
        self.assertEqual(out["d"]["text"], "card done")
        self.assertTrue(all(r["source"] == "draft" and r["gold"] for r in out.values()))

    def test_packets_are_blind_and_deduplicated(self):
        manifest = {"clips": [{"id": "a", "ms": 1000}, {"id": "b", "ms": 2000}]}
        runs = [{"clips": {"a": {"text": "one"}, "b": {"text": "two"}}}, {"clips": {"a": {"text": "one"}, "b": {"text": "too"}}}]
        texts = gold.packets(manifest, runs, {"a": "One."}, size=1)
        self.assertEqual(len(texts), 2)
        self.assertEqual(texts[0].count("one"), 1)
        self.assertIn("POLISHED", texts[0])
        self.assertNotIn("POLISHED", texts[1])


if __name__ == "__main__":
    unittest.main()
