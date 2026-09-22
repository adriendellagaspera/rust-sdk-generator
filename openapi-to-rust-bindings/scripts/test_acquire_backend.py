"""Pin and checkout regression tests without network calls."""

from __future__ import annotations

from pathlib import Path
import subprocess
import tempfile
import unittest

from acquire_backend import acquire


class AcquisitionTests(unittest.TestCase):
    def test_local_git_checkout_resolves_and_verifies_an_immutable_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            source.mkdir()
            subprocess.run(["git", "init", str(source)], check=True, capture_output=True)
            subprocess.run(
                ["git", "-C", str(source), "-c", "user.name=Fixture",
                 "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "initial"],
                check=True, capture_output=True,
            )
            sha = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
            from acquire_backend import run
            original = run

            def local_run(*args: object) -> str:
                command = list(args)
                if command[:2] == ["git", "-C"] and command[3:6] == ["remote", "add", "origin"]:
                    command[-1] = str(source)
                return original(*command)

            import unittest.mock
            with unittest.mock.patch("acquire_backend.run", side_effect=local_run):
                self.assertEqual(
                    acquire("gpu-cli/openapi-to-rust", sha, root / "checkout", sha),
                    sha,
                )
                with self.assertRaisesRegex(RuntimeError, "pin mismatch"):
                    acquire("gpu-cli/openapi-to-rust", sha, root / "mismatch", "0" * 40)

    def test_ref_and_destination_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "already").mkdir()
            (root / "already" / "keep").write_text("unchanged")
            for repository, ref, expected in [
                ("unsafe/repo/extra", "main", None),
                ("gpu-cli/openapi-to-rust", "-bad-ref", None),
                ("gpu-cli/openapi-to-rust", "main", "abc"),
            ]:
                with self.assertRaises(ValueError):
                    acquire(repository, ref, root / "new", expected)
            with self.assertRaisesRegex(ValueError, "not empty"):
                acquire("gpu-cli/openapi-to-rust", "main", root / "already")


if __name__ == "__main__":
    unittest.main()
