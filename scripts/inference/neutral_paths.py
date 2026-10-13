"""Neutral build paths for the shipped native runtimes (MW-RELEASE-14).

Compilers embed `__FILE__` and debug paths, and CMake/FFmpeg receipts record
the build directory and toolchain. Builds map their source, build and home
folders to fixed prefixes; receipts and notices copied into a release replace
any remaining home folder with `~`. scripts/security/personal_paths.py checks
the result.
"""
import json
from pathlib import Path
import re

# /mnt/<drive>/Users first, so it is not left as `/mnt/c~`.
HOMES = re.compile(r'/mnt/[a-z]/Users/[^/\\\s"\']+|/(?:home|Users)/[^/\\\s"\']+'
                   r'|\b[A-Za-z]:(?:\\\\|\\|/)(?:Users|Documents and Settings)(?:\\\\|\\|/)[^/\\\s"\']+')


def roots(**named):
    """[(path, neutral)]: the home catch-all, then each named build folder."""
    result = [(str(Path.home()), '~')]
    for name, path in named.items():
        if path:
            result.append((str(Path(path).resolve()), '/writer/' + name.replace('_', '-')))
    return result


def prefix_flags(mapping, clang_cl=False):
    """Compiler flags for __FILE__, debug info and coverage paths.

    clang uses the longest matching prefix, so specific folders beat `~`."""
    flags = [f'-ffile-prefix-map={path}={neutral}' for path, neutral in mapping]
    return [f'/clang:{flag}' for flag in flags] if clang_cl else flags


def neutral(text):
    return HOMES.sub('~', text)


def neutral_json(value):
    """A receipt with every string's home folder replaced."""
    if isinstance(value, str):
        return neutral(value)
    if isinstance(value, list):
        return [neutral_json(item) for item in value]
    if isinstance(value, dict):
        return {key: neutral_json(item) for key, item in value.items()}
    return value


def write_receipt(path, receipt):
    Path(path).write_text(json.dumps(neutral_json(receipt), indent=2) + '\n')


def copy_text(source, destination):
    """Copy a text receipt/notice (CMakeCache, config.mak) without home folders."""
    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    data = Path(source).read_bytes().decode('utf-8', 'surrogateescape')
    destination.write_bytes(neutral(data).encode('utf-8', 'surrogateescape'))


def neutralize_tree(root):
    """Rewrite retained text notices that still name a home folder; return them."""
    changed = []
    for file in sorted(Path(root).rglob('*')):
        if not file.is_file() or file.is_symlink():
            continue
        try:
            text = file.read_bytes().decode('utf-8')
        except UnicodeDecodeError:
            continue
        if HOMES.search(text):
            file.write_text(neutral(text))
            changed.append(file.relative_to(root).as_posix())
    return changed
