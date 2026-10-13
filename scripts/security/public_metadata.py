"""Verify public signing hashes without suppressing a file, directory or rule.

The scanner still runs over every input. A redacted generic match is eligible
only when its exact capture maps into a verified hash property's byte range.
All other matches, decoded findings and unsupported structures stay unreviewed.
"""
import base64
import collections
import hashlib
import plistlib
import re
import struct
from xml.parsers.expat import ExpatError

# Replay only the pinned Gitleaks 8.30.1 generic rule to locate its capture.
# https://github.com/gitleaks/gitleaks/blob/v8.30.1/config/gitleaks.toml
# No rule or allowlist is passed to Gitleaks by this module.
GENERIC = re.compile(rb'''(?i)[\w.-]{0,50}?(?:access|auth|(?-i:[Aa]pi|API)|credential|creds|key|passw(?:or)?d|secret|token)(?:[ \t\w.-]{0,20})[\s'"]{0,3}(?:=|>|:{1,3}=|\|\||:|=>|\?=|,)[\x60'"\s=]{0,5}([\w.=-]{10,150}|[a-z0-9][a-z0-9+/]{11,}={0,3})(?:[\x60'"\s;]|\\[nr]|$)''')
DATA = re.compile(rb'<data>([ \t\r\nA-Za-z0-9+/=]*)</data>')


def data_regions(xml, offset, expected):
    found, regions = [], []
    for match in DATA.finditer(xml):
        text = re.sub(rb'\s', b'', match.group(1))
        decoded = base64.b64decode(text, validate=True)
        found.append(decoded)
        regions.append((offset + match.start(1), offset + match.end(1), text))
    if collections.Counter(found) != collections.Counter(expected):
        return []
    return regions


def resource_regions(raw, payload, containing):
    """Return only hash-data spans whose referenced payload bytes reproduce them."""
    try:
        document = plistlib.loads(raw)
        if not isinstance(document, dict) or set(document) - {'files', 'files2', 'rules', 'rules2'}:
            return []
        expected = []
        for section in ('files', 'files2'):
            entries = document.get(section, {})
            if not isinstance(entries, dict):
                return []
            for relative, value in entries.items():
                candidate = containing.parent.parent / relative
                path = candidate.resolve()
                if candidate.is_symlink() or not path.is_relative_to(payload.resolve()) or not path.is_file():
                    return []
                contents = path.read_bytes()
                if isinstance(value, bytes):
                    if value != hashlib.sha1(contents).digest():
                        return []
                    expected.append(value)
                elif isinstance(value, dict) and not set(value) - {'hash', 'hash2', 'optional'}:
                    for key, hasher in [('hash', hashlib.sha1), ('hash2', hashlib.sha256)]:
                        if key in value:
                            if value[key] != hasher(contents).digest():
                                return []
                            expected.append(value[key])
                else:
                    return []
        return data_regions(raw, 0, expected) if expected else []
    except (ValueError, TypeError, KeyError, OSError, ExpatError, plistlib.InvalidFileException):
        return []


def signature_regions(raw):
    """Reproduce code pages and CMS-declared CodeDirectory hashes; emit no values.

    This checks public hash content. Publisher/CMS signature trust remains the
    separate native codesign/notarization/Gatekeeper acceptance requirement.
    """
    try:
        if raw[:4] != b'\xcf\xfa\xed\xfe' or len(raw) < 32:
            return []
        commands = struct.unpack_from('<I', raw, 16)[0]
        cursor, signature_offset, signature_size = 32, None, None
        if commands > 4096:
            return []
        for _ in range(commands):
            command, size = struct.unpack_from('<II', raw, cursor)
            if size < 8 or cursor + size > len(raw):
                return []
            if command == 0x1d:
                if size != 16 or signature_offset is not None:
                    return []
                signature_offset, signature_size = struct.unpack_from('<II', raw, cursor + 8)
            cursor += size
        if signature_offset is None or signature_offset < cursor or signature_offset + signature_size > len(raw):
            return []
        signature = raw[signature_offset:signature_offset + signature_size]
        magic, length, count = struct.unpack_from('>III', signature)
        if magic != 0xfade0cc0 or length > len(signature) or count > 4096 or 12 + 8*count > length:
            return []
        hashes, cms, cms_offset = [], None, None
        for index in range(count):
            slot, offset = struct.unpack_from('>II', signature, 12 + 8*index)
            magic, size = struct.unpack_from('>II', signature, offset)
            if offset < 12 + 8*count or size < 8 or offset + size > length:
                return []
            blob = signature[offset:offset + size]
            if magic == 0xfade0c02:
                if size < 44:
                    return []
                version, _, hash_offset, identifier, specials, pages, limit = struct.unpack_from('>7I', blob, 8)
                width, kind, _, power = struct.unpack_from('>4B', blob, 36)
                hasher, expected_width = {1: (hashlib.sha1, 20), 2: (hashlib.sha256, 32),
                                          3: (hashlib.sha256, 20), 4: (hashlib.sha384, 48)}.get(kind, (None, None))
                if hasher is None or width != expected_width or power > 30 or limit > signature_offset:
                    return []
                if version >= 0x20100 and (size < 48 or struct.unpack_from('>I', blob, 44)[0]):
                    return []  # Scattered CodeDirectories need their own reviewed verifier.
                if hash_offset - specials*width < 44 or not 44 <= identifier < hash_offset - specials*width:
                    return []
                page_bytes = 1 << power
                if pages != (limit + page_bytes - 1)//page_bytes or hash_offset + pages*width > size:
                    return []
                for page in range(pages):
                    calculated = hasher(raw[page*page_bytes:min((page+1)*page_bytes, limit)]).digest()[:width]
                    if blob[hash_offset+page*width:hash_offset+(page+1)*width] != calculated:
                        return []
                hashes.append(hasher(blob).digest()[:20])
            elif slot == 0x10000:
                if magic != 0xfade0b01 or cms is not None:
                    return []
                cms, cms_offset = blob[8:], signature_offset + offset + 8
        if not hashes or cms is None:
            return []
        regions = []
        for match in re.finditer(rb'<\?xml.*?</plist>', cms, re.S):
            doc = plistlib.loads(match.group())
            if not isinstance(doc, dict) or set(doc) != {'cdhashes'} or not isinstance(doc['cdhashes'], list):
                continue
            if collections.Counter(doc['cdhashes']) != collections.Counter(hashes):
                return []
            regions.extend(data_regions(match.group(), cms_offset + match.start(), hashes))
        return regions
    except (ValueError, TypeError, KeyError, IndexError, struct.error, ExpatError, plistlib.InvalidFileException):
        return []


def public_findings(rows, scanned, regions, spans=None):
    """Return fingerprints matching verified property spans, not just their values.

    `spans` maps generated binary-string offsets back to original file bytes:
    (output start, output end, input start, UTF stride). Plain files use identity.
    """
    if not regions:
        return set()
    candidates = {}
    for match in GENERIC.finditer(scanned):
        start, end = match.span(1)
        if spans is None:
            original_start, original_end = start, end
        else:
            mapped = next((s for s in spans if s[0] <= start and end <= s[1]), None)
            if mapped is None:
                continue
            a, _, source, stride = mapped
            original_start = source + (start-a)*stride
            original_end = source + (end-a)*stride
        value = match.group(1)
        if not any(a <= original_start and original_end <= b and value == expected
                   for a, b, expected in regions):
            continue
        trimmed_end = match.end()
        while trimmed_end > match.start() and scanned[trimmed_end-1] in b' \t\r\n':
            trimmed_end -= 1
        # Pinned Gitleaks uses these column offsets; require exact position too.
        start_line = scanned[:match.start()].count(b'\n') + 1
        end_line = scanned[:trimmed_end].count(b'\n') + 1
        start_column = match.start() - scanned.rfind(b'\n', 0, match.start()) + 1
        end_column = trimmed_end - scanned.rfind(b'\n', 0, trimmed_end)
        redacted = match.group().replace(value, b'REDACTED').decode('ascii').strip()
        candidates[(start_line, end_line, start_column, end_column, redacted)] = True
    verified = set()
    for row in rows:
        if row['RuleID'] != 'generic-api-key' or row.get('Tags'):
            continue
        key = (row.get('StartLine'), row.get('EndLine'), row.get('StartColumn'),
               row.get('EndColumn'), row.get('Match', '').strip())
        if key in candidates:
            verified.add(row['Fingerprint'])
    return verified
