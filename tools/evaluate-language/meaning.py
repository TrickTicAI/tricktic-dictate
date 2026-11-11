#!/usr/bin/env python3
"""Meaning checks for polished dictations (MW-123). Measurement only.

Compares what the polish was given (the recognized text) with what it
returned and names the ways the meaning may have changed: a sentence or
several content words dropped, a negation or a person flipped, a question
turned into a statement. Numbers are excluded: the review found that number
formatting ("five" -> "5", "two thirty six" -> "236") is where the in-app check
refused correct polish. These are heuristics for a rate per configuration on
the scorecard; nothing here refuses text in the app.
"""
import re

from personal import words

NEGATIONS = {"not", "no", "never", "nothing", "none", "nobody", "nowhere", "neither", "nor", "without"}
FIRST = {"i", "i'm", "i've", "i'll", "i'd", "me", "my", "mine", "myself", "we", "we're", "we've", "we'll", "we'd", "us", "our", "ours"}
SECOND = {"you", "you're", "you've", "you'll", "you'd", "your", "yours", "yourself", "yourselves"}
NUMBER_WORDS = {
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
    "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty", "thirty", "forty",
    "fifty", "sixty", "seventy", "eighty", "ninety", "hundred", "thousand", "million", "billion", "first", "second",
    "third", "half", "point", "percent",
}
# Words whose loss changes no request: articles, fillers, connectives and the
# like. Everything else counts as a content word.
FUNCTION = {
    "a", "an", "the", "and", "or", "but", "so", "then", "that", "this", "these", "those", "it", "it's", "its", "is",
    "are", "was", "were", "be", "been", "being", "am", "to", "of", "in", "on", "at", "for", "with", "from", "by", "as",
    "if", "just", "like", "um", "uh", "uhm", "erm", "hmm", "okay", "ok", "yeah", "well", "actually", "basically",
    "really", "very", "also", "too", "there", "here", "what", "which", "who", "do", "does", "did", "have", "has",
    "had", "will", "would", "can", "could", "should", "shall", "may", "might", "must", "know", "mean", "kind", "sort",
    "right", "now", "oh", "all", "some", "any", "about", "up", "out", "into", "over", "again", "only", "even",
    "still", "please", "let's", "gonna", "wanna", "want", "get", "got",
} | FIRST | SECOND
SENTENCE = re.compile(r"[^.!?\n]+[.!?]*")
DROPPED_SHARE = 0.15
DROPPED_MIN = 3
SENTENCE_MIN_WORDS = 4


def number(token):
    return any(c.isdigit() for c in token) or token in NUMBER_WORDS


def negations(tokens):
    return sum(t in NEGATIONS or t.endswith("n't") for t in tokens)


def content(tokens):
    return [t for t in tokens if t not in FUNCTION and not number(t) and t not in NEGATIONS and not t.endswith("n't")]


def tokens(text):
    """Words without numbers, "cannot" as "can't", and a stutter ("won't
    won't won't") as one word: removing a repeat is formatting."""
    out = []
    for token in words(text):
        token = "can't" if token == "cannot" else token
        if not number(token) and (not out or out[-1] != token):
            out.append(token)
    return out


def changes(given, polished):
    """Reasons the polish may have changed the meaning of `given`, in a fixed
    order: dropped_sentence, dropped_content, negation, person, question."""
    before, after = tokens(given), tokens(polished)
    reasons = []
    kept = set(after)
    for sentence in SENTENCE.findall(given or ""):
        said = content(words(sentence))
        if len(said) >= SENTENCE_MIN_WORDS and sum(t in kept for t in said) < len(said) / 2:
            reasons.append("dropped_sentence")
            break
    remaining = {}
    for token in content(after):
        remaining[token] = remaining.get(token, 0) + 1
    missing = 0
    said = content(before)
    for token in said:
        if remaining.get(token):
            remaining[token] -= 1
        else:
            missing += 1
    if missing >= DROPPED_MIN and missing >= DROPPED_SHARE * len(said):
        reasons.append("dropped_content")
    if negations(before) != negations(after):
        reasons.append("negation")
    if any((sum(t in group for t in before) > 0) != (sum(t in group for t in after) > 0) for group in (FIRST, SECOND)):
        reasons.append("person")
    if (polished or "").count("?") < (given or "").count("?"):
        reasons.append("question")
    return reasons
