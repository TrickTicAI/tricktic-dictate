#!/usr/bin/env python3
"""Score one private receipt against an explicit private reference; no audio access."""
import argparse
import hashlib
import json
from pathlib import Path
import unicodedata


def words(text):
    # Case-fold and remove punctuation; preserve Hebrew letters and diacritics.
    return ''.join(c for c in unicodedata.normalize('NFC', text).casefold()
                   if not unicodedata.category(c).startswith('P')).split()


def edits(reference, hypothesis):
    row = list(range(len(hypothesis) + 1))
    for i, expected in enumerate(reference, 1):
        next_row = [i]
        for j, actual in enumerate(hypothesis, 1):
            next_row.append(min(row[j] + 1, next_row[-1] + 1,
                                row[j - 1] + (expected != actual)))
        row = next_row
    return row[-1]


def scripts(text):
    return sorted({script for script in ('HEBREW', 'LATIN')
                   if any(unicodedata.name(c, '').startswith(script + ' ')
                          and 'LETTER' in unicodedata.name(c, '') for c in text)})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('receipt', type=Path)
    parser.add_argument('reference', type=Path, help='UTF-8 text, kept outside source')
    args = parser.parse_args()
    receipt_bytes = args.receipt.read_bytes()
    receipt = json.loads(receipt_bytes.decode('utf-8-sig'))
    reference_bytes = args.reference.read_bytes()
    expected = words(reference_bytes.decode('utf-8-sig'))
    raw = receipt.get('raw_text')
    if not expected or not isinstance(raw, str) or not raw.strip():
        parser.error('A nonempty explicit reference and recorded original transcript are required.')
    distance = edits(expected, words(raw))
    missing = sorted(set(scripts(reference_bytes.decode('utf-8-sig'))) - set(scripts(raw)))
    print(json.dumps({
        'receipt_sha256': hashlib.sha256(receipt_bytes).hexdigest(),
        'reference_sha256': hashlib.sha256(reference_bytes).hexdigest(),
        'normalization': 'NFC, casefold, remove Unicode punctuation, whitespace words',
        'reference_words': len(expected), 'word_edits': distance,
        'word_error_rate': distance / len(expected),
        'missing_reference_scripts': missing,
        'scope': 'single supplied sample; not a representative accuracy benchmark',
    }, indent=2))
    if missing:
        raise SystemExit(1)
