"""Read-only summary tests using the report schema emitted by verify.py."""
from contextlib import redirect_stderr, redirect_stdout
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import summarize_verification as summary


class SummaryTests(unittest.TestCase):
    def setUp(self):
        root = Path(__file__).resolve().parent.parent / "work/workflow-efficiency-20261017"
        root.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="summary-", dir=root)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.report_dir = self.root / "report"
        self.report_dir.mkdir()
        self.path = self.report_dir / "result.json"
        self.data = {
            "schema_version": 1, "profile": "contracts", "scope": "Selected contract checks; not full acceptance.",
            "run_id": "fixture", "status": "passed", "platform": "windows",
            "source": {"head": "a" * 40, "dirty_diff_sha256": "b" * 64,
                       "note": "Dirty diff digest covers tracked files; untracked sources are not included."},
            "steps": [{"id": "core", "status": "passed", "passed_count": 3}],
            "commands": [{"argv": ["never-execute-me"], "cwd": str(self.root),
                          "stdout": str(self.report_dir / "command-000.stdout.log"),
                          "stderr": str(self.report_dir / "command-000.stderr.log"),
                          "timeout_seconds": 30, "exit_code": 0}],
            "protected_author_files": {}, "artifacts": [], "native_host": "fixture",
        }
        self.write()

    def write(self):
        self.path.write_text(json.dumps(self.data), encoding="utf-8")

    def run_cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = summary.main([str(self.path), *args])
        return code, out.getvalue(), err.getvalue()

    def failed(self):
        self.data["status"] = "failed"
        self.data["steps"] = [{"id": "core", "status": "failed", "reason": "Exact/filter test selection is absent or ambiguous; zero tests is not a pass"},
                              {"id": "native", "status": "skipped", "reason": "Windows only"},
                              {"id": "later", "status": "not_run", "reason": "earlier step failed"}]
        self.data["commands"][0].update(exit_code=101)
        self.write()

    def test_success_directory_and_file(self):
        for path in (self.path, self.report_dir):
            resolved, report = summary.load_report(path)
            text, code = summary.summarize(resolved, report)
            self.assertEqual(code, 0)
            self.assertIn("passed=1, failed=0", text)
            self.assertIn("contracts", text)
            self.assertIn("a" * 40, text)
            self.assertIn("untracked sources are not covered", text)
            self.assertIn("successful commands omitted: 1", text)

    def test_failure_skip_not_run_exit_and_exact_diagnostic(self):
        self.failed()
        code, text, err = self.run_cli()
        self.assertEqual((code, err), (1, ""))
        for expected in ("Step failed: core", "zero tests is not a pass", "Step skipped: native", "Step not_run: later", "exit=101", "timeout_seconds=30", "stderr:", "failed commands=1"):
            self.assertIn(expected, text)

    def test_passed_status_without_execution_evidence_is_incomplete(self):
        scenarios = [
            ([], [], ("no passed steps", "no recorded commands")),
            ([{"id": "platform-only", "status": "skipped", "reason": "Windows only"}],
             self.data["commands"], ("no passed steps",)),
            (self.data["steps"], [], ("no recorded commands",)),
        ]
        for steps, commands, reasons in scenarios:
            with self.subTest(steps=steps, commands=commands):
                self.data.update(steps=steps, commands=commands)
                self.write()
                code, text, err = self.run_cli()
                self.assertEqual((code, err), (1, ""))
                self.assertIn("reported status: passed | summary: incomplete", text)
                self.assertIn("This does not establish a product failure.", text)
                for reason in reasons:
                    self.assertIn(reason, text)

    def test_genuinely_executed_report_passes_and_default_budget_is_6000(self):
        self.assertEqual(self.run_cli()[0], 0)
        self.data["steps"] += [
            {"id": "skip-%d" % i, "status": "skipped", "reason": "x" * 200}
            for i in range(100)]
        self.write()
        path, data = summary.load_report(self.path)
        api_text, api_code = summary.summarize(path, data)
        cli_code, cli_text, _ = self.run_cli()
        self.assertEqual((api_code, cli_code), (0, 0))
        self.assertEqual(api_text, cli_text)
        self.assertLessEqual(len(api_text), 6000)
        self.assertIn("summary: passed", api_text)
        self.assertRegex(api_text, r"Detail entries omitted: [1-9]")

    def test_timeout_launch_error_and_report_error(self):
        self.failed()
        self.data["commands"][0].update(exit_code=None, error="timeout", cleanup_error="owned process did not terminate")
        self.data["error"] = "Protected author files changed; no automatic restore was attempted"
        self.write()
        code, text, _ = self.run_cli()
        self.assertEqual(code, 1)
        self.assertIn("error=timeout", text)
        self.assertIn("Cleanup error: owned process did not terminate", text)
        self.assertIn("Report error: Protected author files changed", text)
        self.data["commands"][0]["error"] = "file not found"
        self.write()
        self.assertIn("file not found", self.run_cli()[1])

    def test_failed_step_with_successful_command_still_fails(self):
        self.failed()
        self.data["status"] = "passed"
        self.data["commands"][0]["exit_code"] = 0
        self.write()
        code, text, _ = self.run_cli()
        self.assertEqual(code, 1)
        self.assertIn("reported status: passed | summary: failed/incomplete", text)
        self.assertIn("last command context", text)
        self.assertIn("stderr:", text)

    def test_preparation_failure_before_steps(self):
        self.data["status"] = "failed"
        self.data["error"] = "Foreign CARGO_BUILD_TARGET is not a native verification environment"
        self.data["steps"] = [{"id": "core", "status": "not_run"}]
        self.data["commands"] = []
        self.write()
        self.assertEqual(self.run_cli()[0], 1)
        self.assertIn("Foreign CARGO_BUILD_TARGET", self.run_cli()[1])

    def test_excerpts_preserve_diagnostics_and_are_opt_in(self):
        self.failed()
        (self.report_dir / "command-000.stderr.log").write_text("error[E0308]: mismatched types\nexpected `u32`, found `&str`\n", encoding="utf-8")
        (self.report_dir / "command-000.stdout.log").write_text("FAILED tests::fixture\n", encoding="utf-8")
        self.assertNotIn("mismatched types", self.run_cli()[1])
        text = self.run_cli("--excerpts")[1]
        self.assertIn("error[E0308]: mismatched types\nexpected `u32`, found `&str`", text)
        self.assertIn("FAILED tests::fixture", text)
        self.assertIn("untrusted", text)

    def test_external_absolute_and_relative_logs_are_not_read(self):
        self.failed()
        outside = self.root / "secret.log"
        outside.write_text("DO NOT READ THIS", encoding="utf-8")
        self.data["commands"][0].update(stderr=str(outside), stdout="../secret.log")
        self.write()
        original_open = Path.open
        def guarded(path, *args, **kwargs):
            self.assertNotEqual(path.resolve(), outside)
            return original_open(path, *args, **kwargs)
        with patch.object(Path, "open", guarded):
            text = self.run_cli("--excerpts")[1]
        self.assertEqual(text.count("external log; not read"), 2)
        self.assertNotIn("DO NOT READ THIS", text)

    def test_resolved_link_escape_is_not_read(self):
        outside = self.root / "secret.log"
        outside.write_text("SECRET", encoding="utf-8")
        link = self.report_dir / "link.log"
        try:
            link.symlink_to(outside)
        except OSError:
            self.skipTest("Creating symlinks is unavailable in this environment")
        self.assertEqual(summary.excerpt(link, self.report_dir, 100), "[external log; not read]")

    def test_relative_local_missing_and_control_char_logs(self):
        self.failed()
        self.data["commands"][0].update(stderr="err.log", stdout="missing.log")
        (self.report_dir / "err.log").write_bytes(b"normal\n\x1b[31mred\x00")
        self.write()
        text = self.run_cli("--excerpts")[1]
        self.assertIn("normal\n\\u001b[31mred\\u0000", text)
        self.assertIn("log missing or not a regular file", text)

    def test_bounded_output_and_excerpts_explain_omissions(self):
        self.failed()
        (self.report_dir / "command-000.stderr.log").write_bytes(b"x" * (summary.MAX_LOG_BYTES + 100))
        text = self.run_cli("--excerpts", "--excerpt-chars", "10")[1]
        self.assertIn("excerpt truncated", text)
        self.assertIn("unread bytes remain", text)
        self.data["steps"] *= 200
        self.write()
        text = self.run_cli("--excerpts", "--max-chars", "1000")[1]
        self.assertLessEqual(len(text), 1000)
        self.assertIn("failed=200", text)
        self.assertRegex(text, r"detail entries omitted: [1-9]|Detail entries omitted: [1-9]")

    def test_malformed_missing_and_unknown_schema(self):
        for raw in ("{", "[]", '{"schema_version":2}', '{"schema_version":true}',
                    '{"schema_version":1,"schema_version":1}', '{"schema_version":NaN}'):
            self.path.write_text(raw, encoding="utf-8")
            code, text, err = self.run_cli()
            self.assertEqual((code, text), (2, ""))
            self.assertIn("Verification summary error:", err)
        self.path.unlink()
        self.assertEqual(self.run_cli()[0], 2)

    def test_invalid_status_or_command_is_rejected(self):
        for change in (lambda d: d.pop("commands"), lambda d: d.update(status="running"),
                       lambda d: d["steps"][0].update(status="unknown"),
                       lambda d: d["commands"][0].update(exit_code=True),
                       lambda d: d["commands"][0].update(timeout_seconds=0),
                       lambda d: d["commands"][0].update(timeout_seconds=10 ** 1000),
                       lambda d: d.update(source={})):
            original = copy.deepcopy(self.data)
            change(self.data)
            self.write()
            self.assertEqual(self.run_cli()[0], 2)
            self.data = original

    def test_report_byte_cap(self):
        with patch.object(summary, "MAX_REPORT_BYTES", 10):
            self.assertIn("exceeds 10 bytes", self.run_cli()[2])

    def test_no_writes_or_command_execution(self):
        self.failed()
        before = {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in self.root.rglob("*") if p.is_file()}
        original_open = Path.open
        def read_only(path, mode="r", *args, **kwargs):
            self.assertNotRegex(mode, r"[wax+]")
            return original_open(path, mode, *args, **kwargs)
        with patch.object(Path, "open", read_only), patch("subprocess.Popen", side_effect=AssertionError("must never execute")), patch("os.system", side_effect=AssertionError("must never execute")):
            self.assertEqual(self.run_cli("--excerpts")[0], 1)
        after = {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in self.root.rglob("*") if p.is_file()}
        self.assertEqual(before, after)


if __name__ == "__main__":
    unittest.main()
