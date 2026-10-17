#!/usr/bin/env python3
"""Bounded Editor diagnostics using a supplied native executable, without a build."""
import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import platform
import re
import struct
import subprocess
import sys
import uuid
import zlib

import review_snapshot
import verify

VIEWS = ("render", "shader", "history", "engine", "settings", "game", "scene", "assets", "systems", "custom")
MENUS = ("File", "Edit", "Tools", "Windows")
WORKFLOW_FLAGS = ("new_templates_preserve_engine", "copy_retains_author_document", "unassigned_asset_copied",
                  "windows_reuses_current_workspace", "exported_player_pixels_equal", "original_project_unchanged")
ENV_KEYS = ("MODULE_ENGINE_GUI_BACKEND", "MODULE_ENGINE_WINDOW_SYSTEM", "MODULE_ENGINE_DIAGNOSTIC_USER_ROOT",
            "MODULE_ENGINE_UI_SCREENSHOT_TO", "MODULE_ENGINE_CAPTURE_VIEW", "MODULE_ENGINE_CAPTURE_MENU",
            "TEMP", "TMP", "TMPDIR", "DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "WGPU_BACKEND")
LIMITS = ("Source inventory does not prove the supplied executable was built from these sources. "
          "Native screenshots are application readbacks, not DWM/compositor or foreground input evidence. "
          "Workflow uses existing project services, egui menu checks and offscreen model/Player rendering. "
          "No latency, cold-start, hardware Linux, native file-picker or full product acceptance.")


class DiagnosticError(verify.VerificationError):
    pass


class DiagnosticTimeout(DiagnosticError):
    pass


def contained(path, parent):
    path = Path(path).resolve()
    parent = Path(parent).resolve()
    if path.is_relative_to(parent):
        return path
    if os.name == "nt":
        # Rust canonical paths may retain \\?\ while pathlib retains the ordinary
        # spelling of the allowed directory. Compare filesystem identity without
        # rewriting extended/UNC/device names or undoing resolve's link handling.
        for ancestor in (path, *path.parents):
            try:
                if ancestor.samefile(parent):
                    return path
            except OSError:
                continue
    raise DiagnosticError("Path escapes allowed directory: " + str(path))


def png_evidence(path, parent):
    """Validate complete bounded RGBA8 PNG data, including CRC and zlib termination."""
    original = Path(path)
    path = contained(original, parent)
    if original.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= 64 * 1024 * 1024:
        raise DiagnosticError("Missing, linked or oversized PNG: " + str(path))
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise DiagnosticError("Invalid PNG signature: " + str(path))
    offset, dimensions, compressed, ended, after_idat = 8, None, bytearray(), False, False
    while offset < len(data):
        if len(data) - offset < 12:
            raise DiagnosticError("Truncated PNG chunk")
        size, = struct.unpack_from(">I", data, offset)
        kind = data[offset + 4:offset + 8]
        end = offset + 12 + size
        if end > len(data):
            raise DiagnosticError("Truncated PNG payload")
        payload = data[offset + 8:end - 4]
        crc, = struct.unpack_from(">I", data, end - 4)
        if zlib.crc32(kind + payload) & 0xffffffff != crc:
            raise DiagnosticError("PNG CRC mismatch")
        if offset == 8 and kind != b"IHDR":
            raise DiagnosticError("PNG must begin with IHDR")
        if kind == b"IHDR":
            if dimensions or size != 13:
                raise DiagnosticError("Invalid PNG IHDR")
            width, height, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", payload)
            if not (0 < width <= 8192 and 0 < height <= 8192 and width * height <= 16 * 1024 * 1024
                    and (depth, color, compression, filtering, interlace) == (8, 6, 0, 0, 0)):
                raise DiagnosticError("PNG must have bounded positive RGBA8 noninterlaced dimensions")
            dimensions = (width, height)
        elif kind == b"IDAT":
            if not dimensions or after_idat:
                raise DiagnosticError("Invalid PNG IDAT order")
            compressed.extend(payload)
        elif kind == b"IEND":
            if size or not compressed or end != len(data):
                raise DiagnosticError("Invalid PNG IEND or trailing data")
            ended = True
        else:
            if compressed:
                after_idat = True
            if not kind[0] & 32 and kind != b"PLTE":
                raise DiagnosticError("Unsupported critical PNG chunk")
        offset = end
    if not ended or dimensions is None:
        raise DiagnosticError("Incomplete PNG")
    width, height = dimensions
    expected = height * (1 + width * 4)
    decoder = zlib.decompressobj()
    try:
        pixels = decoder.decompress(bytes(compressed), expected + 1)
    except zlib.error as error:
        raise DiagnosticError("Invalid PNG zlib stream") from error
    if (len(pixels) != expected or not decoder.eof or decoder.unused_data or decoder.unconsumed_tail
            or any(pixels[row * (1 + width * 4)] > 4 for row in range(height))):
        raise DiagnosticError("PNG has incomplete or invalid scanlines")
    return {"path": str(path), "sha256": verify.digest(path), "width": width, "height": height,
            "format": "RGBA8", "size_bytes": len(data)}


def graphics_evidence(log, system, requested_window):
    marker = "ModuleEngine GUI initialized: GuiGraphicsInfo {"
    found = []
    with Path(log).open("rb") as stream:
        while raw := stream.readline(verify.MAX_LINE + 1):
            if len(raw) > verify.MAX_LINE:
                # Consume an oversized line without retaining it in memory.
                while raw and not raw.endswith(b"\n"):
                    raw = stream.readline(verify.MAX_LINE + 1)
                continue
            line = raw.decode("utf-8", errors="replace").strip()
            if marker in line:
                if len(line) > 16384 or not line.endswith("}"):
                    raise DiagnosticError("Malformed or oversized initialized graphics line")
                found.append(line[line.index(marker):])
    if len(found) != 1:
        raise DiagnosticError("Expected exactly one actual initialized GUI graphics line")
    raw = found[0]
    fields = {}
    for key in ("renderer", "backend", "window_system", "requested_renderer", "requested_gpu_backend", "requested_window_system"):
        matches = re.findall(r'(?:\{|,)\s*' + key + r': (?:Some\()?"([^"\r\n]*)"\)?(?=,|\s*\})', raw)
        if len(matches) != 1:
            raise DiagnosticError("Missing or ambiguous graphics field: " + key)
        fields[key] = matches[0]
    backend = "Dx12" if system == "windows" else "Vulkan"
    if (fields["renderer"] != "Wgpu" or fields["backend"] != backend
            or fields["requested_renderer"] != "Wgpu"
            or fields["requested_gpu_backend"].lower() != backend.lower()
            or fields["requested_window_system"] != requested_window):
        raise DiagnosticError("Initialized GUI or reported request does not match required native WGPU policy")
    windows = ("Windows",) if system == "windows" else ("X11", "Wayland")
    if fields["window_system"] not in windows or (requested_window != "auto" and fields["window_system"].lower() != requested_window):
        raise DiagnosticError("Actual GUI window system does not match request/platform")
    return {"raw_initialized_line": raw, **fields}


def environment(temp, personal, window):
    env = os.environ.copy()  # Retain normal DLL/loader, wrapper and Cargo environment.
    removed = sorted(key for key in env if key == "EFRAME_SCREENSHOT_TO" or key.startswith("MODULE_ENGINE_CAPTURE_")
                     or key in ("MODULE_ENGINE_UI_SCREENSHOT_TO", "MODULE_ENGINE_SHOW_CUSTOM"))
    for key in removed:
        env.pop(key)
    env.update(TEMP=str(temp), TMP=str(temp), TMPDIR=str(temp), MODULE_ENGINE_GUI_BACKEND="wgpu",
               MODULE_ENGINE_WINDOW_SYSTEM=window, MODULE_ENGINE_DIAGNOSTIC_USER_ROOT=str(personal))
    return env, removed


class Runner:
    def __init__(self, root, args, output, fixture, env, report, executor):
        self.root, self.args, self.output, self.fixture = root, args, output, fixture
        self.env, self.report, self.executor = env, report, executor

    def immutable(self):
        if verify.digest(self.args.editor) != self.report["editor"]["sha256"]:
            raise DiagnosticError("Supplied executable changed during diagnostics")
        expected = self.report.get("fixture", {}).get("sha256")
        if expected and (not self.fixture.is_file() or verify.digest(self.fixture) != expected):
            raise DiagnosticError("Generated fixture changed during diagnostics")

    def command(self, argv, env=None):
        self.immutable()
        child_env = env or self.env
        record = self.executor(list(map(str, argv)), self.root, child_env.copy(), self.args.timeout,
                               self.output / ("command-%03d" % len(self.report["commands"])))
        record["environment"] = {key: child_env[key] for key in ENV_KEYS if key in child_env}
        record["status"] = "timed_out" if record.get("error") == "timeout" else (
            "passed" if record.get("exit_code") == 0 and not record.get("error") else "failed")
        self.report["commands"].append(record)
        if record["status"] == "timed_out":
            raise DiagnosticTimeout("Editor command timed out; see owned-process cleanup and logs")
        self.immutable()
        if record["status"] != "passed":
            raise DiagnosticError("Editor command failed; see command logs")
        return record

    def run(self):
        template = "--new-puzzle" if self.args.scenario == "capture" else "--new-game"
        self.report["fixture"] = {"path": str(self.fixture), "template": template}
        if self.fixture.exists():
            raise DiagnosticError("Fixture path already exists")
        self.command([self.args.editor, template, self.fixture])
        if not self.fixture.is_file() or self.fixture.is_symlink():
            raise DiagnosticError("Editor did not emit a fresh fixture")
        self.report["fixture"]["sha256"] = verify.digest(self.fixture)
        if self.args.scenario in ("workflow", "both"):
            folder = self.output / "workflow"
            if folder.exists():
                raise DiagnosticError("Workflow evidence path already exists")
            self.command([self.args.editor, "--editor-ui-check", self.fixture, folder])
            path = contained(folder / "workflow-report.json", folder)
            if path.is_symlink() or not path.is_file() or path.stat().st_size > 1024 * 1024:
                raise DiagnosticError("Missing or oversized workflow report")
            data = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(data, dict):
                raise DiagnosticError("Workflow report must be an object")
            self.report["workflow"] = {"path": str(path), "sha256": verify.digest(path), "report": data}
            menus = data.get("menus_clicked")
            if (data.get("passed") is not True or any(data.get(key) is not True for key in WORKFLOW_FLAGS)
                    or not isinstance(menus, dict) or set(menus) != set(MENUS) or any(value is not True for value in menus.values())
                    or type(data.get("model_pixels_changed")) is not int or data["model_pixels_changed"] <= 0
                    or type(data.get("model_primitives")) is not int or data["model_primitives"] <= 0):
                raise DiagnosticError("Workflow report does not confirm required services, menus and model pixels")
            case = contained(data["case_folder"], folder)
            for name in ("relocated-model.png", "player-model.png"):
                self.report["captures"].append(png_evidence(case / name, folder))
        if self.args.scenario in ("capture", "both"):
            capture = self.output / "native-ui.png"
            if capture.exists():
                raise DiagnosticError("Native capture path already exists")
            env = dict(self.env, MODULE_ENGINE_UI_SCREENSHOT_TO=str(capture), MODULE_ENGINE_CAPTURE_VIEW=self.args.view)
            if self.args.menu:
                env["MODULE_ENGINE_CAPTURE_MENU"] = self.args.menu
            record = self.command([self.args.editor, self.fixture], env)
            self.report["captures"].append(png_evidence(capture, self.output))
            self.report["graphics"] = graphics_evidence(record["stderr"], verify.PLATFORM, self.args.window_system)


def parser():
    result = argparse.ArgumentParser(description=__doc__, epilog="Default timeout is 180 seconds per command; use --timeout 1800 explicitly for workflow builds.")
    result.add_argument("--editor", required=True, type=Path, help="Absolute path to an existing native Editor executable")
    result.add_argument("--scenario", choices=("capture", "workflow", "both"), default="capture")
    result.add_argument("--view", choices=VIEWS, default="scene")
    result.add_argument("--menu", choices=MENUS)
    result.add_argument("--window-system", choices=("auto", "x11", "wayland"), default="auto")
    result.add_argument("--timeout", type=float, default=180)
    result.add_argument("--output-root", type=Path)
    return result


def main(argv=None, root=None, executor=None, snapshotter=None):
    root = Path(root or Path(__file__).resolve().parents[1]).resolve()
    args = parser().parse_args(argv)
    try:
        if verify.PLATFORM not in ("windows", "linux"):
            raise DiagnosticError("Only native Windows/Linux diagnostics are supported")
        if not args.editor.is_absolute():
            raise DiagnosticError("--editor must be an absolute native executable path")
        if not math.isfinite(args.timeout) or args.timeout <= 0:
            raise DiagnosticError("--timeout must be positive and finite")
        arch = {"amd64": "x86_64", "arm64": "aarch64"}.get(platform.machine().lower(), platform.machine().lower())
        args.editor = verify.validate_executable(args.editor, args.editor.parent, arch + ("-pc-windows-msvc" if verify.PLATFORM == "windows" else "-unknown-linux-gnu"))
        output_base = contained(root / "outputs", root)
        output_root = contained(args.output_root or output_base / "editor-diagnostics", output_base)
        nonce = uuid.uuid4().hex[:12]
        run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + nonce
        output = output_root / run_id
        output.mkdir(parents=True, exist_ok=False)
        report = {"schema_version": 1, "run_id": run_id, "status": "failed", "started_at": verify.utc(),
                  "platform": verify.PLATFORM, "scenario": args.scenario, "view": args.view, "menu": args.menu,
                  "window_system": args.window_system, "timeout_seconds": args.timeout,
                  "editor": {"path": str(args.editor), "sha256": verify.digest(args.editor)},
                  "window_request": {"inner_size": [1580, 940], "minimum_inner_size": [1100, 720]},
                  "commands": [], "captures": [], "errors": [], "limits": LIMITS}
        try:
            with verify.run_lease(root / "work/verification/launcher.lock"):
                baseline = None
                runner = None
                try:
                    data, _ = verify.load_registry(root / "scripts/verification.json")
                    patterns = sorted(set(data["protected"] + [".user", "editor-recent-projects.json", "editor-layout.json"]))
                    baseline = verify.protected_snapshot(root, patterns)
                    report["protected_before"] = baseline
                    report["source"] = (snapshotter or review_snapshot.capture)(root)
                    work = contained(root / "work/verification/editor-diagnostics" / run_id, root)
                    temp = contained(root / ".work/v" / nonce, root)
                    personal = work / "personal"
                    personal.mkdir(parents=True, exist_ok=False)
                    temp.mkdir(parents=True, exist_ok=False)
                    report.update(work_directory=str(work), temporary_directory=str(temp), personal_root=str(personal))
                    env, removed = environment(temp, personal, args.window_system)
                    report["environment"] = {key: env[key] for key in ENV_KEYS if key in env}
                    report["removed_inherited_capture_variables"] = removed
                    runner = Runner(root, args, output, work / "fixture.json", env, report, executor or verify.execute)
                    runner.run()
                    report["status"] = "passed"
                except DiagnosticTimeout as error:
                    report["status"] = "timed_out"
                    report["errors"].append(str(error))
                except (verify.VerificationError, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
                    report["errors"].append(str(error))
                finally:
                    checks = []
                    if baseline is not None:
                        def preservation():
                            after = verify.protected_snapshot(root, patterns)
                            report["protected_after"] = after
                            report["protected_unchanged"] = after == baseline
                            if after != baseline:
                                raise DiagnosticError("Protected author files changed; no automatic restore was attempted")
                        checks.append(preservation)
                    if runner:
                        def immutable():
                            report["editor"]["sha256_after"] = verify.digest(args.editor) if args.editor.is_file() else None
                            if "fixture" in report:
                                report["fixture"]["sha256_after"] = verify.digest(runner.fixture) if runner.fixture.is_file() else None
                            runner.immutable()
                        checks.append(immutable)
                    if "source" in report:
                        def source_preservation():
                            after = (snapshotter or review_snapshot.capture)(root)
                            report["source_after"] = {key: after[key] for key in ("head", "source_digest")}
                            report["source_unchanged"] = after["source_digest"] == report["source"]["source_digest"]
                            if not report["source_unchanged"]:
                                raise DiagnosticError("Source snapshot changed during diagnostics")
                        checks.append(source_preservation)
                    for check in checks:
                        try:
                            check()
                        except (verify.VerificationError, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
                            if report["status"] != "timed_out":
                                report["status"] = "failed"
                            report["errors"].append(str(error))
        except (verify.VerificationError, OSError, ValueError) as error:
            report["errors"].append(str(error))
        report["finished_at"] = verify.utc()
        result = output / "result.json"
        result.write_text(json.dumps(report, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"status": report["status"], "report": str(result), "commands": len(report["commands"]),
                          "captures": len(report["captures"]), "errors": [error[:512] for error in report["errors"][:8]]}))
        return 0 if report["status"] == "passed" else 1
    except (verify.VerificationError, OSError, ValueError) as error:
        print("Editor diagnostics: " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
