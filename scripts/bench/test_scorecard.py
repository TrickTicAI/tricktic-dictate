#!/usr/bin/env python3
"""The recognition and polish scorecard commands coexist after integration."""
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import scorecard


class CombinedScorecardTests(unittest.TestCase):
    def test_derive_then_compare_recognition_and_polish(self):
        with tempfile.TemporaryDirectory() as directory:
            gold = Path(directory)
            scorecard.save(gold / "manifest.json", {"clips": [{"id": "a", "ms": 3000}]})
            scorecard.save(gold / "references.json", [
                {"recording_id": "a", "text": "merge the card", "gold": True}])
            scorecard.save(gold / "terms.json", ["merge", "card"])
            clip = {"text": "merge the card Thank you.", "polished": "merge the card",
                    "segments": [{"text": "merge the card", "start": 0},
                                 {"text": " Thank you.", "start": 2.8}]}
            scorecard.save(gold / "runs/base.json", {"name": "base", "config": {}, "clips": {"a": clip}})
            with contextlib.redirect_stdout(io.StringIO()):
                with patch("sys.argv", ["scorecard", "--gold", directory, "derive", "base", "--tails", "0.5"]):
                    scorecard.main()
                with patch("sys.argv", ["scorecard", "--gold", directory, "score", "base-tails0.5", "--against", "base"]):
                    scorecard.main()
            derived = scorecard.load(gold / "runs/base-tails0.5.json")
            self.assertEqual(derived["clips"]["a"]["text"], "merge the card")
            result = scorecard.load(gold / "scores/base-tails0.5.json")
            self.assertLess(result["comparison"]["WER"]["difference"], 0)
            self.assertEqual(result["comparison"]["polished WER"]["difference"], 0)
            # Polish no longer drops the raw hallucinated tail after derivation.
            self.assertEqual(result["comparison"]["meaning changed"]["difference"], -1)

    def test_polish_commands_remain_available(self):
        for command in ["polish-plan", "polish-run"]:
            with self.subTest(command=command), patch("sys.argv", ["scorecard", command, "--help"]):
                with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(SystemExit) as result:
                    scorecard.main()
                self.assertEqual(result.exception.code, 0)


if __name__ == "__main__":
    unittest.main()
