#!/usr/bin/env python3
"""Compatibility entry point for the shared pinned FFmpeg source build."""
from pathlib import Path
import subprocess
import sys

if __name__ == '__main__':
    sys.exit(subprocess.call([sys.executable, str(Path(__file__).with_name('build-ffmpeg.py')),
                             *sys.argv[1:], '--target', 'macos']))
