#!/usr/bin/env python3
"""Explicit, sequential verification profiles. Plan/list never start processes."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import signal
import struct
import subprocess
import sys
import time
import uuid

MAX_LINE = 1024 * 1024
PLATFORM = {"Windows": "windows", "Linux": "linux"}.get(platform.system(), "unsupported")


class VerificationError(Exception):
    pass


def utc():
    return datetime.now(timezone.utc).isoformat()


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def source_snapshot(root):
    result = {}
    for key, argv in [("head", ["git", "rev-parse", "HEAD"]),
                      ("dirty_diff_sha256", ["git", "diff", "--no-ext-diff", "--binary", "HEAD", "--"])]:
        try:
            value = subprocess.run(argv, cwd=root, capture_output=True, timeout=30)
            if value.returncode:
                result[key + "_error"] = value.stderr.decode("utf-8", errors="replace").strip()
            else:
                result[key] = value.stdout.decode().strip() if key == "head" else hashlib.sha256(value.stdout).hexdigest()
        except (OSError, subprocess.TimeoutExpired) as error:
            result[key + "_error"] = str(error)
    result["note"] = "Dirty diff digest covers tracked files; untracked sources are not included. Protected author inventory is recorded separately."
    return result


def load_registry(path):
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    if data.get("schema_version") != 1:
        raise VerificationError("Unsupported registry schema")
    steps = data.get("steps", [])
    by_id = {step["id"]: step for step in steps}
    if len(by_id) != len(steps):
        raise VerificationError("Duplicate step id")
    for step in steps:
        if step.get("kind") not in ("command", "tests") or not step.get("purpose"):
            raise VerificationError("Invalid step kind/purpose")
        if step["kind"] == "command":
            if not step.get("argv") or not all(isinstance(v, str) for v in step["argv"]):
                raise VerificationError("Command argv must be a string array")
        elif step.get("target_kind") not in ("lib", "bin") or not step.get("package"):
            raise VerificationError("Invalid Rust test target")
        if step.get("exact") and not step.get("filter"):
            raise VerificationError("Exact tests require a name")
        if step.get("timeout", 300) <= 0:
            raise VerificationError("Timeout must be positive")
    for profile in data.get("profiles", {}).values():
        ids = profile.get("steps", [])
        if len(set(ids)) != len(ids) or any(i not in by_id or by_id[i].get("excluded") for i in ids):
            raise VerificationError("Unknown, duplicate or excluded profile step")
        if not ids and not profile.get("unavailable"):
            raise VerificationError("Empty executable profile")
    return data, by_id


def plan(data, by_id, name, system=PLATFORM):
    if name not in data["profiles"]:
        raise VerificationError("Unknown profile: " + name)
    profile = data["profiles"][name]
    return {"profile": name, "scope": profile["scope"],
            "unavailable": profile.get("unavailable"),
            "steps": [dict(by_id[i], status="planned" if system in by_id[i].get("platforms", ["windows", "linux"])
                           else "skipped", reason="platform not supported" if system not in by_id[i].get("platforms", ["windows", "linux"]) else None)
                      for i in profile["steps"]]}


def protected_snapshot(root, patterns):
    files = set()
    for pattern in patterns:
        for path in root.glob(pattern):
            files.update(path.rglob("*") if path.is_dir() and not path.is_symlink() else [path])
    result = {}
    for path in sorted(files):
        if path.is_symlink():
            result[str(path.relative_to(root))] = "symlink:" + os.readlink(path)
        elif path.is_file():
            result[str(path.relative_to(root))] = digest(path)
    return result


def native_host(text, system=PLATFORM, machine=None):
    hosts = re.findall(r"^host: (\S+)$", text, re.MULTILINE)
    arch = (machine or platform.machine()).lower()
    arch = {"amd64": "x86_64", "arm64": "aarch64"}.get(arch, arch)
    if len(hosts) != 1 or not re.fullmatch(r"[A-Za-z0-9_.-]+", hosts[0]):
        raise VerificationError("rustc did not identify one safe host")
    host = hosts[0]
    if system not in ("windows", "linux") or not host.startswith(arch + "-") or ("-" + system + "-") not in host:
        raise VerificationError("Selected rustc is not native to this OS/architecture")
    return host


def validate_executable(path, target_root, host):
    path, target_root = Path(path).resolve(strict=True), Path(target_root).resolve(strict=True)
    if not path.is_file() or not path.is_relative_to(target_root):
        raise VerificationError("Cargo executable escapes target directory or is not a file")
    with path.open("rb") as stream:
        header = stream.read(64)
        arch = host.split("-", 1)[0]
        if "-windows-" in host:
            offset = struct.unpack_from("<I", header, 60)[0] if len(header) == 64 else 0
            if header[:2] != b"MZ" or not 64 <= offset <= MAX_LINE:
                raise VerificationError("Cargo product is not a native PE executable")
            stream.seek(offset)
            pe = stream.read(26)
            valid = len(pe) == 26 and pe[:4] == b"PE\0\0" and struct.unpack_from("<H", pe, 4)[0] == {"x86_64": 0x8664, "aarch64": 0xaa64}.get(arch) and struct.unpack_from("<H", pe, 24)[0] == 0x20b
        else:
            valid = len(header) == 64 and header[:6] == b"\x7fELF\x02\x01" and struct.unpack_from("<H", header, 16)[0] in (2, 3) and struct.unpack_from("<H", header, 18)[0] == {"x86_64": 62, "aarch64": 183}.get(arch) and os.access(path, os.X_OK)
        if not valid:
            raise VerificationError("Cargo executable has the wrong native machine/header")
    return path


def select_artifact(log, selector, target_root, host, exit_code):
    products, finishes = set(), []
    with Path(log).open("rb") as stream:
        while raw := stream.readline(MAX_LINE + 1):
            if len(raw) > MAX_LINE:
                raise VerificationError("Cargo JSON line exceeds 1 MiB")
            if not raw.lstrip().startswith(b"{"):
                continue  # Cargo allows non-JSON compiler/plugin stdout.
            try:
                value = json.loads(raw)
            except (ValueError, UnicodeError) as error:
                raise VerificationError("Malformed Cargo JSON") from error
            if value.get("reason") == "build-finished":
                finishes.append(value.get("success"))
            if value.get("reason") != "compiler-artifact":
                continue
            target = value.get("target", {})
            manifest = value.get("manifest_path")
            if (value.get("package_id") == selector["package_id"] and manifest
                    and Path(manifest).resolve() == selector["manifest"]
                    and target.get("name") == selector["name"]
                    and selector["kind"] in target.get("kind", [])
                    and value.get("profile", {}).get("test") is selector["test"]
                    and value.get("executable")):
                products.add(value["executable"])
    if exit_code != 0 or len(finishes) != 1 or finishes[0] is not True or len(products) != 1:
        raise VerificationError("Cargo requires exit 0, one successful build-finished and one selected executable")
    return validate_executable(next(iter(products)), target_root, host)


def discover_tests(text):
    return [line[:-6] for line in text.splitlines() if line.endswith(": test")]


@contextmanager
def run_lease(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+b") as stream:
        if path.stat().st_size == 0:
            stream.write(b"0")
            stream.flush()
        stream.seek(0)
        try:
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise VerificationError("Another verification launcher owns this repository's sharedGPU/build lease") from error
        try:
            yield
        finally:
            stream.seek(0)
            if os.name == "nt":
                msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(stream, fcntl.LOCK_UN)


def execute(argv, cwd, env, timeout, prefix):
    """Only terminate the process tree started here; no process-name cleanup."""
    record = {"argv": list(map(str, argv)), "cwd": str(cwd), "started_at": utc(), "timeout_seconds": timeout}
    stdout, stderr = prefix.with_suffix(".stdout.log"), prefix.with_suffix(".stderr.log")
    record.update(stdout=str(stdout), stderr=str(stderr))
    started = time.monotonic()
    with stdout.open("wb") as out, stderr.open("wb") as err:
        try:
            flags = (subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW) if os.name == "nt" else 0
            child = subprocess.Popen(argv, cwd=cwd, env=env, stdout=out, stderr=err,
                                     start_new_session=os.name != "nt", creationflags=flags)
        except OSError as error:
            record.update(exit_code=None, error=str(error), duration_seconds=time.monotonic() - started, finished_at=utc())
            return record
        try:
            child.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
            record["error"] = "interrupted" if isinstance(error, KeyboardInterrupt) else "timeout"
            try:
                if os.name == "nt":
                    taskkill = str(Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32/taskkill.exe")
                    cleanup = subprocess.run([taskkill, "/PID", str(child.pid), "/T", "/F"], capture_output=True, timeout=15)
                    record["cleanup_exit_code"] = cleanup.returncode
                    if cleanup.returncode:
                        record["cleanup_error"] = cleanup.stderr.decode("utf-8", errors="replace")
                        child.kill()
                else:
                    os.killpg(child.pid, signal.SIGKILL)
            except (OSError, subprocess.TimeoutExpired) as cleanup_error:
                record["cleanup_error"] = str(cleanup_error)
                child.kill()
            try:
                child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                record["cleanup_error"] = "owned process did not terminate"
        record["exit_code"] = child.poll()
    record.update(duration_seconds=time.monotonic() - started, finished_at=utc())
    return record


class Runner:
    def __init__(self, root, data, selected_plan, output, temp, timeout=None, executor=execute):
        self.root, self.data, self.plan = root, data, selected_plan
        self.output, self.executor, self.override_timeout = output, executor, timeout
        self.env = os.environ.copy()  # Preserve the normal loader/toolchain environment.
        self.env.update(TEMP=str(temp), TMP=str(temp), TMPDIR=str(temp), CARGO_INCREMENTAL="0")
        self.commands, self.products, self.metadata, self.host = [], {}, None, None
        self.baseline = protected_snapshot(root, data["protected"])

    def command(self, argv, timeout):
        prefix = self.output / ("command-%03d" % len(self.commands))
        record = self.executor(argv, self.root, self.env.copy(), self.override_timeout or timeout, prefix)
        self.commands.append(record)
        if record.get("error") or record.get("exit_code") != 0:
            raise VerificationError("Command failed: " + " ".join(map(str, argv)))
        return Path(record["stdout"])

    def prepare(self):
        text = self.command([self.env.get("RUSTC", "rustc"), "-vV"], 30).read_text(encoding="utf-8")
        self.host = native_host(text)
        requested = self.env.get("CARGO_BUILD_TARGET")
        if requested and requested != self.host:
            raise VerificationError("Foreign CARGO_BUILD_TARGET is not a native verification environment")
        self.toolchain = text
        self.metadata = json.loads(self.command(["cargo", "metadata", "--offline", "--locked", "--no-deps", "--format-version=1"], 60).read_text(encoding="utf-8"))

    def artifact(self, step):
        key = (step["package"], step["target_kind"], step.get("release", False))
        if key not in self.products:
            packages = [p for p in self.metadata["packages"] if p["name"] == step["package"] and p["id"] in self.metadata["workspace_members"] and Path(p["manifest_path"]).resolve().is_relative_to(self.root)]
            if len(packages) != 1:
                raise VerificationError("Metadata did not select one workspace package")
            package = packages[0]
            targets = [t for t in package["targets"] if step["target_kind"] in t["kind"] and t.get("test", True)]
            if len(targets) != 1:
                raise VerificationError("Metadata did not select one test target")
            target = targets[0]
            selector = {"package_id": package["id"], "manifest": Path(package["manifest_path"]).resolve(), "name": target["name"], "kind": step["target_kind"], "test": True}
            argv = ["cargo", "test", "--offline", "--locked", "--no-run", "--message-format=json", "--target", self.host, "-p", step["package"]]
            argv += ["--lib"] if step["target_kind"] == "lib" else ["--bin", target["name"]]
            if step.get("release"):
                argv.append("--release")
            log = self.command(argv, 1200)
            executable = select_artifact(log, selector, self.metadata["target_directory"], self.host, 0)
            self.products[key] = {"path": str(executable), "sha256": digest(executable), "package_id": package["id"], "target": target["name"], "native_host": self.host, "release": step.get("release", False)}
        product = self.products[key]
        if digest(product["path"]) != product["sha256"]:
            raise VerificationError("Selected test executable changed after its Cargo build")
        return product

    def test(self, step):
        product = self.artifact(step)
        common = [product["path"]]
        if step.get("filter"):
            common.append(step["filter"])
        if step.get("exact"):
            common.append("--exact")
        if step.get("ignored"):
            common.append("--ignored")
        listed = discover_tests(self.command(common + ["--list", "--format", "terse"], 30).read_text(encoding="utf-8", errors="replace"))
        if not listed or (step.get("exact") and listed != [step["filter"]]):
            raise VerificationError("Exact/filter test selection is absent or ambiguous; zero tests is not a pass")
        log = self.command(common + ["--nocapture", "--test-threads=1"], step.get("timeout", 300))
        text = log.read_text(encoding="utf-8", errors="replace")
        summaries = re.findall(r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", text, re.MULTILINE)
        if not summaries or tuple(map(int, summaries[-1])) != (len(listed), 0, 0):
            raise VerificationError("Test summary does not confirm the discovered tests passed")
        return {"artifact": product, "tests": listed, "passed_count": len(listed)}

    def run(self):
        results, failed = [], False
        self.prepare()
        for step in self.plan["steps"]:
            result = {"id": step["id"], "purpose": step["purpose"], "status": "not_run"}
            results.append(result)
            if step["status"] == "skipped":
                result.update(status="skipped", reason=step["reason"])
                continue
            if failed:
                result["reason"] = "earlier step failed"
                continue
            print("Running " + step["id"], flush=True)
            result["started_at"] = utc()
            try:
                if step["kind"] == "command":
                    self.command(step["argv"], step["timeout"])
                else:
                    result.update(self.test(step))
                result["status"] = "passed"
            except (VerificationError, OSError, ValueError) as error:
                result.update(status="failed", reason=str(error))
                failed = True
            finally:
                result["finished_at"] = utc()
                if protected_snapshot(self.root, self.data["protected"]) != self.baseline:
                    result.update(status="failed", reason="Protected author files changed; no automatic restore was attempted")
                    failed = True
        return results, failed


def main(argv=None, root=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=["quick", "contracts", "native", "native-tools", "package"], default="quick")
    read_only = parser.add_mutually_exclusive_group()
    read_only.add_argument("--list", action="store_true")
    read_only.add_argument("--plan", action="store_true")
    parser.add_argument("--output-root", type=Path)
    parser.add_argument("--timeout", type=float, help="Explicit per-command timeout override, including compilation; never changes product watchdogs")
    args = parser.parse_args(argv)
    root = Path(root or Path(__file__).resolve().parent.parent).resolve()
    try:
        data, by_id = load_registry(root / "scripts/verification.json")
        selected = plan(data, by_id, args.profile)
        if args.timeout is not None and (not math.isfinite(args.timeout) or args.timeout <= 0):
            raise VerificationError("Timeout must be positive and finite")
        if args.list:
            print(json.dumps({"profiles": data["profiles"], "legacy_routes": data.get("legacy_routes", {}), "ignored_inventory": [s for s in data["steps"] if s.get("ignored")]}, ensure_ascii=False, indent=2))
            return 0
        if args.plan:
            print(json.dumps(selected, ensure_ascii=False, indent=2))
            return 0
        if selected["unavailable"]:
            raise VerificationError(selected["unavailable"])
        if PLATFORM == "unsupported":
            raise VerificationError("Only native Windows/Linux are supported")
        run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
        output_root = (args.output_root or root / "outputs/verification").resolve()
        if not output_root.is_relative_to(root / "outputs"):
            raise VerificationError("Output root must stay under this checkout's outputs directory")
        output = output_root / run_id
        output.mkdir(parents=True, exist_ok=False)
        # MSVC object paths in nested Cargo fixtures can exceed its path limit.
        # Keep disposable input paths short; reports retain the full run ID.
        temp = (root / ".work/v" / run_id.rsplit("-", 1)[1]).resolve()
        if not temp.is_relative_to(root):
            raise VerificationError("Temporary directory escapes this checkout")
        temp.mkdir(parents=True, exist_ok=False)
        runner = Runner(root, data, selected, output, temp, args.timeout)
        implementation = {name: digest(root / name) if (root / name).is_file() else None for name in ["scripts/verify.py", "scripts/verification.json", "verify.ps1", "verify.sh"]}
        report = {"schema_version": 1, "run_id": run_id, "profile": args.profile, "scope": selected["scope"], "started_at": utc(), "platform": PLATFORM, "status": "failed", "temporary_directory": str(temp), "implementation_sha256": implementation, "registry_sha256": implementation["scripts/verification.json"], "source": source_snapshot(root), "protected_author_files": runner.baseline, "steps": [{"id": s["id"], "status": "not_run"} for s in selected["steps"]]}
        try:
            with run_lease(root / "work/verification/launcher.lock"):
                report["steps"], failed = runner.run()
                report["status"] = "failed" if failed else "passed"
        except (VerificationError, OSError, ValueError) as error:
            report["error"] = str(error)
        finally:
            if protected_snapshot(root, data["protected"]) != runner.baseline:
                report.update(status="failed", error="Protected author files changed; no automatic restore was attempted")
            report.update(finished_at=utc(), commands=runner.commands, artifacts=list(runner.products.values()), native_host=runner.host, toolchain=getattr(runner, "toolchain", None))
            (output / "result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            print("Verification report: " + str(output / "result.json"), flush=True)
        return 0 if report["status"] == "passed" else 1
    except (VerificationError, OSError, ValueError) as error:
        print("Verification: " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
