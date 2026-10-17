"""CI path and aggregate policy: no Actions service, Cargo or GPU required."""
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import ci_plan
import ci_linux_dxc


class PolicyTests(unittest.TestCase):
    def test_docs_only_omits_expensive_routes(self):
        selected = ci_plan.select(["docs/current-status.md", "README.md", ".gitignore"])
        self.assertFalse(selected["native"])
        self.assertFalse(selected["package"])

    def test_source_deleted_renamed_and_unknown_paths_require_both_routes(self):
        for path in ["crates/editor/src/game_ui.rs", "Cargo.lock", "scripts/verify.py",
                     "assets/texture.png", "game-project-v4.json", ".cargo/config.toml",
                     ".github/workflows/ci.yml", "future/new-source"]:
            self.assertTrue(ci_plan.select([path])["native"], path)
            self.assertTrue(ci_plan.select([path])["package"], path)

    def test_initial_and_manual_run_require_every_route(self):
        with patch("ci_plan.subprocess.run") as process:
            self.assertTrue(ci_plan.select(ci_plan.changed_paths("0" * 40))["native"])
            self.assertTrue(ci_plan.select(ci_plan.changed_paths(""))["package"])
            process.assert_not_called()

    def test_diff_uses_nul_separation_no_rename_and_no_shell(self):
        with patch("ci_plan.subprocess.run", return_value=subprocess.CompletedProcess([], 0, b"docs/a.md\0crates/space name.rs\0")) as process:
            self.assertEqual(ci_plan.changed_paths("a" * 40), ["crates/space name.rs", "docs/a.md"])
            argv = process.call_args.args[0]
            self.assertIn("-z", argv)
            self.assertIn("--no-renames", argv)
            self.assertNotIn("shell", process.call_args.kwargs)
        with self.assertRaises(ValueError):
            ci_plan.changed_paths("--all")

    def test_diff_failure_never_becomes_docs_only_success(self):
        with patch("ci_plan.subprocess.run", side_effect=subprocess.CalledProcessError(1, "git")):
            with self.assertRaises(subprocess.CalledProcessError):
                ci_plan.changed_paths("a" * 40)

    def test_gate_rejects_every_non_success_prerequisite(self):
        for result in ["failure", "cancelled", "skipped", "", None]:
            for plan, checks in [(result, "success"), ("success", result)]:
                with self.assertRaises(ValueError):
                    ci_plan.check_gate(plan, checks, {"native": "false", "package": "false"})
        ci_plan.check_gate("success", "success", {"native": "true", "package": "false"})
        for selected in [{}, {"native": "", "package": "false"}, {"native": "TRUE", "package": "true"}]:
            with self.assertRaises(ValueError):
                ci_plan.check_gate("success", "success", selected)

    def test_workflow_keeps_unconditional_gate_and_pinned_read_only_actions(self):
        # Structure check complements policy tests; Actions still needs its first real run.
        text = (Path(__file__).resolve().parent.parent / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertNotIn("pull_request_target", text)
        self.assertNotIn("self-hosted", text)
        self.assertNotIn("continue-on-error", text)
        self.assertIn("    if: always()\n    needs: [plan, checks]", text)
        self.assertIn("  contents: read", text)
        self.assertIn("  merge_group:", text)
        self.assertIn("--component rustfmt --component clippy", text)
        self.assertIn("run: python scripts/verify.py --profile clippy --plan", text)
        self.assertIn("run: python scripts/verify.py --profile clippy --timeout 1800", text)
        for line in text.splitlines():
            if "uses:" in line:
                self.assertRegex(line, r"uses: actions/[a-z-]+@[0-9a-f]{40} # v")


class DxcRestoreTests(unittest.TestCase):
    def setUp(self):
        owned = Path(__file__).resolve().parent.parent / "work/verification-tests"
        owned.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="ci-dxc-", dir=owned)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.assertTrue(self.root.is_relative_to(owned.resolve()))
        (self.root / "scripts").mkdir()
        self.archive = self.root / "source.tar.gz"

    def fixture(self, member="bin/dxc", payload=b"pinned compiler"):
        with tarfile.open(self.archive, "w:gz") as stream:
            info = tarfile.TarInfo(member)
            info.size = len(payload)
            stream.addfile(info, io.BytesIO(payload))
        pin = {"dxc": {"url": "https://example.invalid/fixture", "sha256": ci_linux_dxc.sha256(self.archive),
                       "files": {"tools/dxc/bin/dxc": hashlib.sha256(payload).hexdigest()}}}
        (self.root / "scripts/linux-toolchain.json").write_text(json.dumps(pin), encoding="utf-8")
        return pin

    def download(self, _url, destination):
        shutil.copyfile(self.archive, destination)

    def test_archive_and_file_pins_and_no_existing_destination_overwrite(self):
        self.fixture()
        ci_linux_dxc.restore(self.root, self.download)
        self.assertEqual((self.root / ".tools/ci-dxc/bin/dxc").read_bytes(), b"pinned compiler")
        with self.assertRaises(FileExistsError):
            ci_linux_dxc.restore(self.root, self.download)

    def test_corrupt_archive_fails_before_extracting(self):
        self.fixture()
        self.archive.write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "archive checksum"):
            ci_linux_dxc.restore(self.root, self.download)
        self.assertFalse((self.root / ".tools/ci-dxc/bin/dxc").exists())

    def test_corrupt_component_and_tar_traversal_are_rejected(self):
        pin = self.fixture()
        pin["dxc"]["files"]["tools/dxc/bin/dxc"] = "0" * 64
        (self.root / "scripts/linux-toolchain.json").write_text(json.dumps(pin), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "component checksum"):
            ci_linux_dxc.restore(self.root, self.download)
        # Separate owned root so rejection cannot be caused by the existing destination.
        nested = self.root / "traversal"
        (nested / "scripts").mkdir(parents=True)
        pin = self.fixture(member="../outside.txt")
        (nested / "scripts/linux-toolchain.json").write_text(json.dumps(pin), encoding="utf-8")
        with self.assertRaises(tarfile.FilterError):
            ci_linux_dxc.restore(nested, self.download)
        self.assertFalse((nested / ".tools/outside.txt").exists())


if __name__ == "__main__":
    unittest.main()
