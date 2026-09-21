"""Offline regression checks for fail-closed prebuilt LLVM installation."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import bootstrap_llvm as bootstrap


class PrebuiltTests(unittest.TestCase):
    def test_corrupt_download_is_rejected_before_extraction_or_execution(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "archive.tar.xz").write_bytes(b"corrupt download")
            with patch.object(bootstrap.subprocess, "run") as run:
                with self.assertRaisesRegex(SystemExit, "checksum mismatch"):
                    bootstrap.install(root, "Linux", "archive.tar.xz", "0" * 64)
                run.assert_not_called()
            self.assertFalse((root / "installation").exists())

    def test_cache_without_matching_provenance_is_not_trusted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            installed = root / "installation"
            installed.mkdir()
            (installed / "zeb-archive.json").write_text(json.dumps({"archive": "wrong", "sha256": "wrong"}))
            with self.assertRaisesRegex(SystemExit, "unrecognized cached"):
                bootstrap.install(root, "Windows", "full-windows.tar.xz", "1" * 64)

    def test_missing_tool_has_exact_diagnostic_and_no_fallback(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "bin").mkdir()
            (root / "bin/llvm-config.exe").touch()
            result = bootstrap.subprocess.CompletedProcess([], 0, bootstrap.VERSION, "")
            with patch.object(bootstrap.subprocess, "run", return_value=result):
                with self.assertRaisesRegex(SystemExit, "missing required tool bin/opt.exe; no source fallback"):
                    bootstrap.validate(root, "Windows")

    def test_incompatible_binary_reports_loader_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "bin").mkdir()
            (root / "bin/llvm-config").touch()
            with patch.object(bootstrap.subprocess, "run", side_effect=OSError("missing host library")):
                with self.assertRaisesRegex(SystemExit, "llvm-config cannot run.*missing host library.*no source fallback"):
                    bootstrap.validate(root, "Linux")


if __name__ == "__main__":
    unittest.main()
