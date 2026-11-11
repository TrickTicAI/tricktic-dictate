#!/usr/bin/env python3
import unittest

import meaning
import personal


class MeaningTests(unittest.TestCase):
    def test_ordinary_polish_changes_nothing(self):
        given = "um so can you merge the PR and then uh clean up the worktree?"
        self.assertEqual(meaning.changes(given, "Can you merge the PR and then clean up the worktree?"), [])

    def test_numbers_are_excluded(self):
        self.assertEqual(meaning.changes("look at CRM two thirty six and the five cards", "Look at CRM-236 and the 5 cards."), [])

    def test_a_dropped_request_and_dropped_words(self):
        given = "Merge the branch now. After that deploy the newest build to staging and tell the team."
        self.assertEqual(meaning.changes(given, "Merge the branch now."), ["dropped_sentence", "dropped_content"])

    def test_flipped_negation_person_and_question(self):
        self.assertEqual(meaning.changes("don't merge it yet", "Merge it yet."), ["negation"])
        self.assertEqual(meaning.changes("can you check the card", "I will check the card."), ["person"])
        self.assertEqual(meaning.changes("is the card waiting to merge?", "The card is waiting to merge."), ["question"])
        # Adding a question mark the recognizer left out is not a change.
        self.assertEqual(meaning.changes("is the card waiting to merge", "Is the card waiting to merge?"), [])
        # Tidying a stutter or spelling out "cannot" is not a flipped negation.
        self.assertEqual(meaning.changes("I won't won't won't do it, I cannot", "I won't do it, I can't."), [])

    def test_scorecard_reports_the_rate_per_run(self):
        terms = [personal.words(t) for t in personal.PROJECT_TERMS]
        reference = personal.words("is the card waiting to merge")
        same = personal.score_clip(reference, "is the card waiting to merge?", terms, polished="Is the card waiting to merge?")
        flat = personal.score_clip(reference, "is the card waiting to merge?", terms, polished="The card is waiting to merge.")
        self.assertEqual((same["meaning_changed"], flat["meaning"]), (0, ["question"]))
        total = personal.summary({"a": same, "b": flat})
        self.assertEqual((total["meaning_changed"], total["meaning_rate"]), (1, 0.5))
        self.assertEqual(total["meaning_reasons"]["question"], 1)
        interval = personal.paired_bootstrap({"a": same, "b": same}, {"a": same, "b": flat}, "meaning_changed", "one")
        self.assertAlmostEqual(interval["difference"], 0.5)


if __name__ == "__main__":
    unittest.main()
