#!/usr/bin/env python3
"""Linux PR gate. No native release, private model or product credentials needed."""

import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
NODE = "v24.15.0"
RUST = "1.96.0"
GITLEAKS = {
    "x86_64": ("x64", "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb"),
    "aarch64": ("arm64", "e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080"),
}


def install_gitleaks(prefix):
    """Install only the checksum-bound binary, never arbitrary archive paths."""
    if sys.platform != "linux" or platform.machine() not in GITLEAKS:
        raise ValueError("CI supports Linux x86_64/aarch64 only")
    arch, checksum = GITLEAKS[platform.machine()]
    url = f"https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_{arch}.tar.gz"
    with urllib.request.urlopen(url, timeout=120) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != checksum:
        raise ValueError("Gitleaks archive checksum mismatch")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        member = archive.getmember("gitleaks")
        if not member.isfile():
            raise ValueError("Gitleaks archive must contain a regular binary")
        binary = Path(prefix) / "gitleaks"
        binary.write_bytes(archive.extractfile(member).read())
        binary.chmod(0o755)


def run(*argv, capture=False, env=None):
    print("> " + " ".join(map(str, argv)), flush=True)
    result = subprocess.run(argv, cwd=ROOT, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None, env=env)
    return result.stdout.strip() if capture else None


def validate():
    head = os.environ.get("WRITER_VALIDATION_HEAD", "")
    if not re.fullmatch(r"[0-9a-f]{40}", head):
        raise ValueError("WRITER_VALIDATION_HEAD must be the full candidate commit")
    if run("git", "rev-parse", "HEAD", capture=True) != head:
        raise ValueError("checkout does not match the requested candidate")
    run("git", "diff", "--exit-code", "HEAD", "--")
    if run("node", "--version", capture=True) != NODE:
        raise ValueError(f"CI requires Node {NODE}")
    manager = json.loads((ROOT / "package.json").read_text())["packageManager"]
    if not re.fullmatch(r"pnpm@\d+\.\d+\.\d+", manager):
        raise ValueError("packageManager must pin an exact pnpm version")
    env = dict(os.environ, RUSTUP_TOOLCHAIN=RUST, CI="true")
    run("rustup", "toolchain", "install", RUST, "--profile", "minimal",
        "--component", "rustfmt", "--component", "clippy", env=env)
    # Keep package-manager installation inside this job, not the shared host prefix.
    with tempfile.TemporaryDirectory(prefix="writer-ci-pnpm-") as prefix:
        install_gitleaks(prefix)
        run("npm", "install", "--prefix", prefix, "--ignore-scripts",
            "--no-audit", "--no-fund", manager, env=env)
        env["PATH"] = os.pathsep.join([str(Path(prefix) / "node_modules" / ".bin"), prefix, env["PATH"]])
        if run("gitleaks", "version", capture=True, env=env) != "8.30.1":
            raise ValueError("CI requires Gitleaks 8.30.1")
        run("pnpm", "install", "--frozen-lockfile", env=env)
        run("python3", "scripts/dev/test-ci-validate.py", env=env)
        run("pnpm", "validate", env=env)


if __name__ == "__main__":
    try:
        validate()
    except subprocess.CalledProcessError as error:
        sys.exit(error.returncode if error.returncode > 0 else 1)
    except (ValueError, OSError) as error:
        print(f"ci-validate: {error}", file=sys.stderr)
        sys.exit(1)
