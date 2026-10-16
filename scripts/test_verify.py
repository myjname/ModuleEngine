"""Launcher tests use synthetic Cargo JSON/PE/ELF and Python children, never Cargo."""
from contextlib import redirect_stdout, redirect_stderr
import copy
import io
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch

import verify


def executable(path, host):
    path.parent.mkdir(parents=True, exist_ok=True)
    data = bytearray(128)
    if "windows" in host:
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 60, 64)
        data[64:68] = b"PE\0\0"
        struct.pack_into("<H", data, 68, 0x8664)
        struct.pack_into("<H", data, 88, 0x20b)
    else:
        data[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", data, 16, 3)
        struct.pack_into("<H", data, 18, 62)
    path.write_bytes(data)
    path.chmod(0o755)
    return path


class Fixture(unittest.TestCase):
    def setUp(self):
        fixture_root = Path(__file__).resolve().parent.parent / "work/verification-tests"
        fixture_root.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="fixture-", dir=fixture_root)
        if not Path(self.temporary.name).resolve().is_relative_to(fixture_root.resolve()):
            raise AssertionError("Fixture escaped its owned test root")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.data, self.steps = verify.load_registry(Path(__file__).with_name("verification.json"))
        (self.root / "scripts").mkdir()
        (self.root / "scripts/verification.json").write_text(json.dumps(self.data), encoding="utf-8")
        self.host = "x86_64-pc-windows-msvc" if os.name == "nt" else "x86_64-unknown-linux-gnu"
        self.target = self.root / "target with spaces"
        self.binary = executable(self.target / "explicit-host/debug/deps/test-product", self.host)
        self.manifest = self.root / "crates/test/Cargo.toml"
        self.manifest.parent.mkdir(parents=True)
        self.manifest.write_text("", encoding="utf-8")
        self.selector = {"package_id": "path+test", "manifest": self.manifest, "name": "test", "kind": "lib", "test": True}

    def artifact(self, **changes):
        value = {"reason": "compiler-artifact", "package_id": "path+test", "manifest_path": str(self.manifest), "target": {"name": "test", "kind": ["lib"]}, "profile": {"test": True}, "executable": str(self.binary), "fresh": True}
        value.update(changes)
        return value

    def select(self, messages, code=0):
        log = self.root / "cargo.log"
        log.write_bytes(b"compiler stdout is allowed\n" + b"\n".join(json.dumps(m).encode() for m in messages))
        return verify.select_artifact(log, self.selector, self.target, self.host, code)

    def runner(self, selected="contracts", executor=None):
        output, temp = self.root / "out", self.root / "tmp"
        output.mkdir(exist_ok=True)
        temp.mkdir(exist_ok=True)
        runner = verify.Runner(self.root, self.data, verify.plan(self.data, self.steps, selected), output, temp, executor=executor or verify.execute)
        return runner


class RegistryTests(Fixture):
    def test_registry_covers_the_actual_rust_ignored_inventory(self):
        source_root = Path(__file__).resolve().parent.parent / "crates"
        self.assertTrue(source_root.is_dir(), "Run this inventory test in the actual checkout")
        pattern = re.compile(r'#\[ignore[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*fn\s+(\w+)\s*\(')
        actual = []
        for path in source_root.rglob("*.rs"):
            actual.extend(pattern.findall(path.read_text(encoding="utf-8")))
        registered = [s["filter"].rsplit("::", 1)[-1] for s in self.data["steps"] if s.get("ignored")]
        self.assertCountEqual(actual, registered, "Every real #[ignore] needs a purpose and profile/exclusion decision")

    def test_profiles_are_explicit_and_native_never_leaks_into_quick_contracts(self):
        self.assertEqual(set(self.data["profiles"]), {"quick", "contracts", "native", "native-tools", "package"})
        for name in ["quick", "contracts"]:
            chosen = verify.plan(self.data, self.steps, name)["steps"]
            self.assertTrue(chosen)
            self.assertFalse(any(s.get("ignored") or s.get("resources") for s in chosen))
        ignored = [s for s in self.data["steps"] if s.get("ignored")]
        self.assertEqual(len(ignored), 9)
        self.assertEqual({s["id"] for s in ignored if s.get("excluded")}, {"fixture-emitter", "catchup-diagnostic"})
        self.assertEqual(len(verify.plan(self.data, self.steps, "native")["steps"]), 5)
        self.assertEqual(len(verify.plan(self.data, self.steps, "package")["steps"]), 2)
        tools = verify.plan(self.data, self.steps, "native-tools")["steps"]
        self.assertEqual([step["id"] for step in tools], ["native-linker", "native-context", "native-exe-spellings"])
        self.assertFalse(any("sharedGPU" in step.get("resources", []) for step in tools))

    def test_windows_only_is_an_explicit_linux_skip(self):
        chosen = verify.plan(self.data, self.steps, "native", "linux")["steps"]
        skipped = [s for s in chosen if s["status"] == "skipped"]
        self.assertEqual([s["id"] for s in skipped], ["native-exe-spellings"])
        self.assertTrue(skipped[0]["reason"])

    def test_invalid_registry_fails_before_execution(self):
        for change in [lambda d: d.update(schema_version=9),
                       lambda d: d["steps"].append(d["steps"][0]),
                       lambda d: d["profiles"]["quick"]["steps"].append("unknown"),
                       lambda d: d["profiles"]["quick"]["steps"].append("fixture-emitter"),
                       lambda d: d["steps"][0].update(timeout=0)]:
            data = copy.deepcopy(self.data)
            change(data)
            path = self.root / "invalid.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            with self.assertRaises(verify.VerificationError):
                verify.load_registry(path)

    def test_plan_and_list_have_no_processes_or_output_directories(self):
        for option in ["--list", "--plan"]:
            with patch.object(verify.subprocess, "Popen", side_effect=AssertionError("process")), patch.object(verify.subprocess, "run", side_effect=AssertionError("process")), redirect_stdout(io.StringIO()):
                self.assertEqual(verify.main(["--profile", "native", option], self.root), 0)
        self.assertFalse((self.root / "outputs").exists())
        self.assertFalse((self.root / "work").exists())


class ArtifactTests(Fixture):
    def test_fresh_actual_target_product_and_duplicate_identical_artifact(self):
        messages = [self.artifact(), self.artifact(), {"reason": "build-finished", "success": True}]
        self.assertEqual(self.select(messages), self.binary.resolve())

    def test_identity_and_profile_do_not_fall_back_to_a_stale_binary(self):
        for changed in [{"package_id": "dependency"}, {"manifest_path": str(self.root / "elsewhere")},
                        {"target": {"name": "wrong", "kind": ["lib"]}},
                        {"target": {"name": "test", "kind": ["bin"]}}, {"profile": {"test": False}}]:
            with self.assertRaises(verify.VerificationError):
                self.select([self.artifact(**changed), {"reason": "build-finished", "success": True}])

    def test_success_requires_exit_finish_unique_and_existing_product(self):
        finish = {"reason": "build-finished", "success": True}
        other = executable(self.target / "other", self.host)
        cases = [([self.artifact()], 0), ([self.artifact(), finish, finish], 0),
                 ([self.artifact(), {"reason": "build-finished", "success": False}], 0),
                 ([self.artifact(), {"reason": "build-finished", "success": 1}], 0),
                 ([self.artifact(), {"reason": "build-finished", "success": "true"}], 0),
                 ([self.artifact(), finish], 1),
                 ([self.artifact(), self.artifact(executable=str(other)), finish], 0),
                 ([self.artifact(executable=str(self.target / "missing")), finish], 0)]
        for messages, code in cases:
            with self.assertRaises((verify.VerificationError, OSError)):
                self.select(messages, code)

    def test_bad_json_and_oversized_line_are_rejected(self):
        log = self.root / "bad.log"
        for content in [b"{broken", b"{" + b"x" * verify.MAX_LINE]:
            log.write_bytes(content)
            with self.assertRaises(verify.VerificationError):
                verify.select_artifact(log, self.selector, self.target, self.host, 0)

    def test_outside_wrong_machine_and_symlink_escape(self):
        outside = executable(self.root / "outside", self.host)
        with self.assertRaises(verify.VerificationError):
            verify.validate_executable(outside, self.target, self.host)
        data = bytearray(self.binary.read_bytes())
        struct.pack_into("<H", data, 68 if os.name == "nt" else 18, 0)
        self.binary.write_bytes(data)
        with self.assertRaises(verify.VerificationError):
            verify.validate_executable(self.binary, self.target, self.host)
        link = self.target / "escape"
        try:
            link.symlink_to(outside)
        except OSError:
            return  # Windows may lack symlink permission; containment is tested above.
        with self.assertRaises(verify.VerificationError):
            verify.validate_executable(link, self.target, self.host)

    def test_native_host_rejects_duplicate_foreign_and_path_traversal(self):
        self.assertEqual(verify.native_host("host: " + self.host, machine="AMD64"), self.host)
        for text in ["host: ../escape", "host: x86_64-unknown-other-gnu", "host: " + self.host + "\nhost: " + self.host]:
            with self.assertRaises(verify.VerificationError):
                verify.native_host(text, machine="AMD64")


class ExecutionTests(Fixture):
    def test_exact_zero_is_failure_and_later_steps_not_run(self):
        runner = self.runner()
        runner.prepare = lambda: None
        runner.test = lambda step: (_ for _ in ()).throw(verify.VerificationError("zero tests"))
        runner.command = lambda *args: self.root
        with redirect_stdout(io.StringIO()):
            results, failed = runner.run()
        self.assertTrue(failed)
        self.assertEqual(results[1]["status"], "failed")
        self.assertTrue(all(s["status"] == "not_run" for s in results[2:]))

    def test_direct_normal_loader_argv_discovery_summary_and_changed_artifact(self):
        seen = []
        original_path = os.environ.get("PATH")
        def fake(argv, cwd, env, timeout, prefix):
            seen.append((argv, env))
            log = prefix.with_suffix(".stdout.log")
            log.write_text("case::one: test\n" if "--list" in argv else "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n", encoding="utf-8")
            return {"argv": argv, "stdout": str(log), "exit_code": 0}
        with patch.dict(os.environ, {"LD_LIBRARY_PATH": "/normal/dxc/lib", "PATH": "normal tool path"}):
            runner = self.runner(executor=fake)
        runner.artifact = lambda _: {"path": str(self.binary), "sha256": verify.digest(self.binary)}
        step = {"filter": "case::one", "exact": True, "ignored": True}
        result = runner.test(step)
        self.assertEqual(result["passed_count"], 1)
        self.assertTrue(all(a[0][0] == str(self.binary) and a[1]["LD_LIBRARY_PATH"] == "/normal/dxc/lib" for a in seen))
        self.assertIn("--ignored", seen[0][0])
        self.assertIn("--exact", seen[1][0])
        self.assertEqual(os.environ.get("PATH"), original_path)
        runner.products[("p", "lib", False)] = {"path": str(self.binary), "sha256": "stale"}
        # Test the real cached-artifact integrity check, bypassing compilation.
        del runner.artifact
        with self.assertRaises(verify.VerificationError):
            runner.artifact({"package": "p", "target_kind": "lib"})

    def test_zero_discovery_and_false_summary_never_pass(self):
        for discovered in [False, True]:
            def fake(argv, cwd, env, timeout, prefix):
                log = prefix.with_suffix(".stdout.log")
                text = "missing: test\n" if discovered and "--list" in argv else "test result: ok. 0 passed; 0 failed; 1 ignored;\n"
                log.write_text(text, encoding="utf-8")
                return {"stdout": str(log), "exit_code": 0}
            runner = self.runner(executor=fake)
            runner.artifact = lambda _: {"path": str(self.binary)}
            with self.assertRaises(verify.VerificationError):
                runner.test({"filter": "missing", "exact": True})

    def test_author_change_is_not_restored_and_fails(self):
        source = self.root / "game-project-v4.json"
        source.write_text("original", encoding="utf-8")
        runner = self.runner("quick")
        runner.prepare = lambda: None
        def mutate(*_):
            source.write_text("changed", encoding="utf-8")
        runner.command = mutate
        runner.test = lambda _: {}
        with redirect_stdout(io.StringIO()):
            results, failed = runner.run()
        self.assertTrue(failed)
        self.assertEqual(results[0]["status"], "failed")
        self.assertEqual(source.read_text(), "changed")

    def test_lease_is_exclusive_and_released(self):
        lock = self.root / "lease"
        with verify.run_lease(lock):
            with self.assertRaises(verify.VerificationError):
                with verify.run_lease(lock):
                    self.fail("overlapping lease")
        with verify.run_lease(lock):
            pass

    def test_process_failure_missing_program_and_timeout(self):
        missing = verify.execute([str(self.root / "no-program")], self.root, os.environ.copy(), 1, self.root / "missing")
        self.assertIsNone(missing["exit_code"])
        self.assertIn("error", missing)
        failed = verify.execute([sys.executable, "-c", "raise SystemExit(7)"], self.root, os.environ.copy(), 5, self.root / "failed")
        self.assertEqual(failed["exit_code"], 7)
        timed = verify.execute([sys.executable, "-c", "import time; time.sleep(30)"], self.root, os.environ.copy(), 0.1, self.root / "timeout")
        self.assertEqual(timed["error"], "timeout")
        self.assertIsNotNone(timed["exit_code"])

    def test_report_on_preparation_failure_and_output_boundary(self):
        with patch.object(verify, "source_snapshot", return_value={"head": "fixture"}), patch.object(verify.Runner, "prepare", side_effect=verify.VerificationError("fixture preflight failure")), redirect_stdout(io.StringIO()):
            self.assertEqual(verify.main(["--profile", "quick"], self.root), 1)
        reports = list((self.root / "outputs/verification").glob("*/result.json"))
        report = json.loads(reports[0].read_text(encoding="utf-8"))
        self.assertEqual(report["status"], "failed")
        self.assertTrue(all(s["status"] == "not_run" for s in report["steps"]))
        self.assertIn("preflight", report["error"])
        self.assertIn("implementation_sha256", report)
        with redirect_stderr(io.StringIO()):
            self.assertEqual(verify.main(["--output-root", str(self.root / "assets")], self.root), 2)
        self.assertFalse((self.root / "assets").exists())
        for timeout in ["nan", "inf", "0", "-1"]:
            with redirect_stderr(io.StringIO()):
                self.assertEqual(verify.main(["--plan", "--timeout", timeout], self.root), 2)


if __name__ == "__main__":
    unittest.main()
