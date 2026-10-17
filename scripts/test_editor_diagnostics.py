"""Exercise evidence rejection and preservation with engine stand-ins; no Cargo/GPU."""
from contextlib import redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch
import zlib

import editor_diagnostics as diagnostics
import verify


def png():
    # A minimal encoder fixture, independent of the validator and production capture.
    def chunk(kind, payload):
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 1, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"\0\xff\0\0\xff\0\xff\0\xff")) + chunk(b"IEND", b""))


def native_binary(path):
    data = bytearray(128)
    if verify.PLATFORM == "windows":
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 60, 64)
        data[64:68] = b"PE\0\0"
        struct.pack_into("<H", data, 68, 0xaa64 if diagnostics.platform.machine().lower() in ("arm64", "aarch64") else 0x8664)
        struct.pack_into("<H", data, 88, 0x20b)
    else:
        data[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", data, 16, 3)
        struct.pack_into("<H", data, 18, 183 if diagnostics.platform.machine().lower() in ("arm64", "aarch64") else 62)
    path.write_bytes(data)
    path.chmod(0o755)
    return path


class LauncherTests(unittest.TestCase):
    def setUp(self):
        parent = Path(__file__).resolve().parents[1] / "work/editor-diagnostics-tests"
        parent.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=parent, prefix="fixture-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        if not self.root.is_relative_to(parent.resolve()):
            raise AssertionError("Owned test fixture escaped its root")
        (self.root / "scripts").mkdir()
        data, _ = verify.load_registry(Path(__file__).with_name("verification.json"))
        (self.root / "scripts/verification.json").write_text(json.dumps(data), encoding="utf-8")
        self.editor = native_binary(self.root / "editor product.exe")
        self.author = self.root / "author.json"
        self.author.write_bytes(b'{"unknown":null,"authored":"preserve exactly"}\n')
        (self.root / ".user").mkdir()
        (self.root / ".user/editor-recent-projects.json").write_bytes(b"personal recent")
        (self.root / ".user/editor-layout.json").write_bytes(b"personal layout")
        self.original = self.author.read_bytes()
        self.mode = "success"
        self.calls = []

    def snapshot(self, root):
        # Snapshot helper has independent inventory tests; isolate this orchestration
        # from Git and native toolchain setup. Real launch uses review_snapshot.capture.
        return {"schema_version": 1, "head": "test-head", "files": [], "source_digest": "test-source-digest"}

    def child(self, argv, cwd, env, timeout, prefix):
        self.calls.append((argv, env, timeout))
        out, err = prefix.with_suffix(".stdout.log"), prefix.with_suffix(".stderr.log")
        out.write_bytes(b"")
        err.write_bytes(b"")
        record = {"argv": argv, "cwd": str(cwd), "stdout": str(out), "stderr": str(err),
                  "exit_code": 0, "duration_seconds": 0.001, "started_at": "start", "finished_at": "finish"}
        if argv[1] in ("--new-puzzle", "--new-game"):
            Path(argv[2]).write_bytes(b'{"fixture":"new from editor"}')
            if self.mode == "stale":
                (prefix.parent / "native-ui.png").write_bytes(png())
            return record
        if argv[1] == "--editor-ui-check":
            folder = Path(argv[3])
            case = folder / "case-123"
            case.mkdir(parents=True)
            for name in ("relocated-model.png", "player-model.png"):
                (case / name).write_bytes(png())
            report = {"passed": True, "menus_clicked": {"File": True, "Edit": True, "Tools": True, "Windows": True},
                      "new_templates_preserve_engine": True, "copy_retains_author_document": True,
                      "unassigned_asset_copied": True, "windows_reuses_current_workspace": True,
                      "exported_player_pixels_equal": True, "original_project_unchanged": True,
                      "model_pixels_changed": 2, "model_primitives": 1, "case_folder": str(case)}
            if self.mode == "workflow-false":
                report["passed"] = False
            if self.mode == "workflow-incomplete":
                del report["menus_clicked"]["Windows"]
            if self.mode == "workflow-no-pixels":
                report["model_pixels_changed"] = 0
            if self.mode == "workflow-outside":
                report["case_folder"] = str(self.root)
            (folder / "workflow-report.json").write_text(json.dumps(report), encoding="utf-8")
            return record
        capture = Path(env["MODULE_ENGINE_UI_SCREENSHOT_TO"])
        if self.mode == "real-timeout":
            return verify.execute([sys.executable, "-c", "import time; time.sleep(30)"], cwd, env, 0.05, prefix)
        if self.mode == "timeout":
            record.update(error="timeout", exit_code=-1)
            return record
        if self.mode != "missing":
            capture.write_bytes(png())
        if self.mode == "truncated":
            capture.write_bytes(png()[:-5])
        if self.mode == "preservation":
            self.author.write_bytes(b"changed by child")
        if self.mode == "fixture-modified":
            Path(argv[1]).write_bytes(b"changed fixture")
        if self.mode == "exe-modified":
            self.editor.write_bytes(b"changed executable")
        backend = "Dx12" if verify.PLATFORM == "windows" else "Vulkan"
        window = "Windows" if verify.PLATFORM == "windows" else "X11"
        if self.mode == "wrong-backend":
            backend = "Gl"
        if self.mode == "wrong-window":
            window = "Wayland" if verify.PLATFORM == "windows" else "Windows"
        err.write_text('ModuleEngine GUI initialized: GuiGraphicsInfo { renderer: "Wgpu", backend: Some("' + backend
                       + '"), adapter_name: Some("test adapter"), window_system: Some("' + window
                       + '"), requested_renderer: Some("Wgpu"), requested_gpu_backend: Some("'
                       + ("DX12" if verify.PLATFORM == "windows" else "Vulkan")
                       + '"), requested_window_system: Some("auto"), gl_vendor: None, gl_renderer: None }\n', encoding="utf-8")
        if self.mode == "missing-graphics":
            err.write_bytes(b"ModuleEngine GUI requested: Wgpu\n")
        return record

    def launch(self, *extra, mode=None):
        if mode:
            self.mode = mode
        before = set((self.root / "outputs").rglob("result.json"))
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            code = diagnostics.main(["--editor", str(self.editor), *extra], root=self.root,
                                    executor=self.child, snapshotter=self.snapshot)
        added = set((self.root / "outputs").rglob("result.json")) - before
        return code, json.loads(next(iter(added)).read_text(encoding="utf-8")) if added else None

    def test_capture_records_real_evidence_and_isolates_personal_state(self):
        with patch.dict(os.environ, {"EFRAME_SCREENSHOT_TO": "inherited.png", "MODULE_ENGINE_CAPTURE_MENU": "bad",
                                    "MODULE_ENGINE_CAPTURE_ATTACK": "1", "MODULE_ENGINE_SHOW_CUSTOM": "1",
                                    "LD_LIBRARY_PATH": "keep-loader", "CARGO_HOME": "keep-cargo", "RUSTC_WRAPPER": "keep-wrapper"}):
            code, report = self.launch("--view", "assets", "--menu", "Windows")
        self.assertEqual(code, 0)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["fixture"]["template"], "--new-puzzle")
        self.assertEqual(report["source"]["head"], "test-head")
        self.assertEqual((report["captures"][0]["width"], report["captures"][0]["height"]), (2, 1))
        self.assertEqual(report["editor"]["sha256"], verify.digest(self.editor))
        self.assertTrue(report["protected_unchanged"])
        self.assertEqual(self.author.read_bytes(), self.original)
        env = self.calls[-1][1]
        self.assertEqual(env["LD_LIBRARY_PATH"], "keep-loader")
        self.assertEqual(env["CARGO_HOME"], "keep-cargo")
        self.assertEqual(env["RUSTC_WRAPPER"], "keep-wrapper")
        self.assertEqual(env["MODULE_ENGINE_CAPTURE_MENU"], "Windows")
        self.assertEqual(env["MODULE_ENGINE_CAPTURE_VIEW"], "assets")
        self.assertNotIn("EFRAME_SCREENSHOT_TO", env)
        self.assertNotIn("MODULE_ENGINE_CAPTURE_ATTACK", env)
        self.assertNotIn("MODULE_ENGINE_SHOW_CUSTOM", env)
        self.assertNotIn("CARGO_HOME", report["environment"])
        self.assertTrue(Path(env["MODULE_ENGINE_DIAGNOSTIC_USER_ROOT"]).is_dir())
        self.assertTrue(Path(env["MODULE_ENGINE_DIAGNOSTIC_USER_ROOT"]).is_relative_to(self.root / "work/verification/editor-diagnostics"))

    def test_exit_zero_requires_capture_and_actual_native_graphics(self):
        for mode in ("missing", "wrong-backend", "missing-graphics", "wrong-window", "truncated"):
            with self.subTest(mode=mode):
                code, report = self.launch(mode=mode)
                self.assertEqual(code, 1)
                self.assertEqual(report["status"], "failed")
                self.assertTrue(report["errors"])
                self.assertTrue(report["protected_unchanged"])

    def test_workflow_uses_existing_command_and_validates_report_and_captures(self):
        code, report = self.launch("--scenario", "workflow", "--timeout", "1800")
        self.assertEqual(code, 0)
        self.assertEqual(report["fixture"]["template"], "--new-game")
        self.assertEqual(self.calls[-1][0][1], "--editor-ui-check")
        self.assertEqual(self.calls[-1][2], 1800)
        self.assertEqual(len(report["captures"]), 2)
        self.assertNotIn("graphics", report)
        for mode in ("workflow-false", "workflow-incomplete", "workflow-no-pixels", "workflow-outside"):
            with self.subTest(mode=mode):
                code, failed = self.launch("--scenario", "workflow", mode=mode)
                self.assertEqual(code, 1)
                self.assertEqual(failed["status"], "failed")

    def test_both_uses_one_new_game_fixture_and_three_captures(self):
        code, report = self.launch("--scenario", "both")
        self.assertEqual(code, 0)
        self.assertEqual(len(report["captures"]), 3)
        self.assertEqual(len(self.calls), 3)
        self.assertEqual(self.calls[1][0][2], self.calls[2][0][1])

    def test_stale_capture_rejected_before_capture_process_and_fresh_runs_differ(self):
        code, report = self.launch(mode="stale")
        self.assertEqual(code, 1)
        self.assertEqual(len(self.calls), 1)
        self.assertIn("already exists", report["errors"][0])
        _, first = self.launch(mode="success")
        _, second = self.launch()
        self.assertNotEqual(first["run_id"], second["run_id"])
        self.assertNotEqual(first["fixture"]["path"], second["fixture"]["path"])
        self.assertNotEqual(first["captures"][0]["path"], second["captures"][0]["path"])

    def test_timeout_is_not_pass_and_retains_report_after_real_child_cleanup(self):
        for mode in ("timeout", "real-timeout"):
            with self.subTest(mode=mode):
                code, report = self.launch(mode=mode)
                self.assertEqual(code, 1)
                self.assertEqual(report["status"], "timed_out")
                self.assertEqual(report["commands"][-1]["status"], "timed_out")
                self.assertTrue(report["protected_unchanged"])
                self.assertIsNotNone(report["commands"][-1]["exit_code"])

    def test_author_change_is_reported_and_never_restored(self):
        code, report = self.launch(mode="preservation")
        self.assertEqual(code, 1)
        self.assertFalse(report["protected_unchanged"])
        self.assertNotEqual(report["protected_before"], report["protected_after"])
        self.assertEqual(self.author.read_bytes(), b"changed by child")

    def test_executable_and_fixture_must_remain_immutable(self):
        for mode in ("fixture-modified", "exe-modified"):
            with self.subTest(mode=mode):
                native_binary(self.editor)
                code, report = self.launch(mode=mode)
                self.assertEqual(code, 1)
                self.assertTrue(any("changed" in error for error in report["errors"]))

    def test_invalid_paths_arguments_and_nonfinite_timeouts_fail_explicitly(self):
        for extra in (("--output-root", str(self.root / "elsewhere")), ("--timeout", "0"),
                      ("--timeout", "nan"), ("--timeout", "inf")):
            with self.subTest(extra=extra):
                code, report = self.launch(*extra)
                self.assertEqual(code, 2)
                self.assertIsNone(report)
        for extra in (("--view", "unknown"), ("--scenario", "unknown"), ("--menu", "missing")):
            with self.subTest(extra=extra), redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as raised:
                diagnostics.main(["--editor", str(self.editor), *extra], root=self.root)
            self.assertEqual(raised.exception.code, 2)
        with redirect_stderr(io.StringIO()):
            self.assertEqual(diagnostics.main(["--editor", "relative.exe"], root=self.root), 2)

    @unittest.skipUnless(os.name == "nt", "Windows extended filesystem paths")
    def test_extended_windows_paths_accept_same_directory_and_reject_outside(self):
        allowed = self.root / "allowed"
        inside = allowed / "case"
        outside = self.root / "outside/case"
        inside.mkdir(parents=True)
        outside.mkdir(parents=True)
        (inside / "capture.png").write_bytes(png())
        extended_inside = Path("\\\\?\\" + str(inside))
        extended_outside = Path("\\\\?\\" + str(outside))
        self.assertTrue(os.path.samefile(extended_inside.parent, allowed))
        accepted = diagnostics.contained(extended_inside, allowed)
        self.assertTrue(os.path.samefile(accepted, inside))
        # Exercise the actual capture/report path as well as directory containment.
        self.assertEqual(diagnostics.png_evidence(extended_inside / "capture.png", allowed)["width"], 2)
        with self.assertRaises(diagnostics.DiagnosticError):
            diagnostics.contained(extended_outside, allowed)
        with self.assertRaises(diagnostics.DiagnosticError):
            diagnostics.contained(extended_outside / "not-created.png", allowed)

    def test_png_crc_truncation_and_zlib_completeness_are_required(self):
        path = self.root / "image.png"
        original = png()
        path.write_bytes(original)
        self.assertEqual(diagnostics.png_evidence(path, self.root)["width"], 2)
        corrupted = bytearray(original)
        corrupted[-1] ^= 1
        for data in (bytes(corrupted), original[:-12], original + b"trailing", original[:40]):
            with self.subTest(data_length=len(data)):
                path.write_bytes(data)
                with self.assertRaises(diagnostics.DiagnosticError):
                    diagnostics.png_evidence(path, self.root)
        # Correct CRCs with a truncated deflate stream must also be rejected.
        start = original.index(b"IDAT")
        size, = struct.unpack_from(">I", original, start - 4)
        payload = original[start + 4:start + 4 + size - 2]
        broken = (original[:start - 4] + struct.pack(">I", len(payload)) + b"IDAT" + payload
                  + struct.pack(">I", zlib.crc32(b"IDAT" + payload) & 0xffffffff) + original[-12:])
        path.write_bytes(broken)
        with self.assertRaises(diagnostics.DiagnosticError):
            diagnostics.png_evidence(path, self.root)


if __name__ == "__main__":
    unittest.main()
