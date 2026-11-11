#!/usr/bin/env python3
"""Exercise CI admission inputs and failure propagation without builds/network."""

import importlib.util
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("ci_validate", Path(__file__).with_name("ci-validate.py"))
ci = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci)
HEAD = "a" * 40


class ValidationTests(unittest.TestCase):
    def execute(self, fail=None, head=HEAD, node=ci.NODE):
        calls = []

        def command(*args, **kwargs):
            calls.append((args, kwargs))
            if args == fail:
                raise subprocess.CalledProcessError(23, args)
            if args == ("git", "rev-parse", "HEAD"):
                return head
            if args == ("node", "--version"):
                return node
            if args == ("gitleaks", "version"):
                return "8.30.1"

        self.calls = calls
        with patch.dict(os.environ, WRITER_VALIDATION_HEAD=HEAD), patch.object(ci, "run", command), patch.object(ci, "install_gitleaks"):
            ci.validate()
        return calls

    def test_complete_gate_uses_locked_dependencies_and_isolated_pnpm(self):
        calls = self.execute()
        commands = [args for args, _ in calls]
        self.assertIn(("pnpm", "install", "--frozen-lockfile"), commands)
        self.assertEqual(commands[-1], ("pnpm", "validate"))
        env = calls[-1][1]["env"]
        self.assertEqual(env["RUSTUP_TOOLCHAIN"], ci.RUST)
        self.assertEqual(env["CI"], "true")
        self.assertIn("writer-ci-pnpm-", env["PATH"].split(os.pathsep)[0])
        self.assertFalse(Path(env["PATH"].split(os.pathsep)[0]).exists())

    def test_wrong_commit_stops_before_install(self):
        with self.assertRaisesRegex(ValueError, "candidate"):
            self.execute(head="b" * 40)
        self.assertEqual(len(self.calls), 1)

    def test_dirty_checkout_stops_before_install(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.execute(fail=("git", "diff", "--exit-code", "HEAD", "--"))
        self.assertEqual(len(self.calls), 2)

    def test_wrong_node_stops_before_install(self):
        with self.assertRaisesRegex(ValueError, "Node"):
            self.execute(node="v22.0.0")
        self.assertFalse(any(args[0] == "npm" for args, _ in self.calls))

    def test_install_or_test_failure_cannot_pass_gate(self):
        for command in [("pnpm", "install", "--frozen-lockfile"),
                        ("python3", "scripts/dev/test-ci-validate.py"),
                        ("pnpm", "validate")]:
            with self.subTest(command=command), self.assertRaises(subprocess.CalledProcessError) as failure:
                self.execute(fail=command)
            self.assertEqual(failure.exception.returncode, 23)
            self.assertEqual(self.calls[-1][0], command)

    def test_missing_candidate_refuses_execution(self):
        with patch.dict(os.environ, WRITER_VALIDATION_HEAD=""), patch.object(ci, "run") as run:
            with self.assertRaisesRegex(ValueError, "full candidate"):
                ci.validate()
            run.assert_not_called()

    def test_subprocess_failure_is_not_swallowed(self):
        with self.assertRaises(subprocess.CalledProcessError) as failure:
            ci.run("python3", "-c", "raise SystemExit(19)")
        self.assertEqual(failure.exception.returncode, 19)

    def test_scanner_checksum_mismatch_refuses_installation(self):
        with tempfile.TemporaryDirectory() as prefix, patch.object(ci.platform, "machine", return_value="x86_64"), patch.object(ci.urllib.request, "urlopen", return_value=io.BytesIO(b"tampered")):
            with self.assertRaisesRegex(ValueError, "checksum"):
                ci.install_gitleaks(prefix)
            self.assertEqual(list(Path(prefix).iterdir()), [])

    def test_scanner_extracts_only_verified_binary(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            for name, content in [("gitleaks", b"fixture binary"), ("../escape", b"unwanted")]:
                member = tarfile.TarInfo(name)
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))
        data = buffer.getvalue()
        checksum = hashlib.sha256(data).hexdigest()
        with tempfile.TemporaryDirectory() as prefix, patch.object(ci.platform, "machine", return_value="x86_64"), patch.dict(ci.GITLEAKS, x86_64=("x64", checksum)), patch.object(ci.urllib.request, "urlopen", return_value=io.BytesIO(data)):
            ci.install_gitleaks(prefix)
            self.assertEqual([p.name for p in Path(prefix).iterdir()], ["gitleaks"])
            self.assertEqual((Path(prefix) / "gitleaks").read_bytes(), b"fixture binary")

    def test_workflow_trust_and_exact_head_boundary(self):
        workflow = (ci.ROOT / ".github/workflows/validate.yml").read_text()
        self.assertIn("github.event.pull_request.head.repo.full_name == github.repository", workflow)
        self.assertIn("github.event.pull_request.head.sha || github.sha", workflow)
        self.assertIn("ref: ${{ env.WRITER_VALIDATION_HEAD }}", workflow)
        self.assertIn("persist-credentials: false", workflow)
        self.assertIn("--size heavy --wait forever --timeout 7200", workflow)
        self.assertNotIn("pull_request_target:", workflow)
        self.assertNotIn("  push:", workflow)
        self.assertNotIn("continue-on-error", workflow)


if __name__ == "__main__":
    unittest.main()
