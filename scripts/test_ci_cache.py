"""Cache admission fixtures: no Cargo, network, native builds or Actions service."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import ci_cache


class CacheTests(unittest.TestCase):
    def setUp(self):
        owned = Path(__file__).resolve().parent.parent / "work/verification-tests"
        owned.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="ci-cache-", dir=owned)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve() / "checkout"
        (self.root / "scripts").mkdir(parents=True)
        self.host = "x86_64-unknown-linux-gnu"
        self.pin = {"rust": {"version": "1.99.0", "target": self.host}}
        (self.root / "scripts/linux-toolchain.json").write_text(json.dumps(self.pin))
        self.env = {"RUNNER_OS": "Linux", "RUNNER_ARCH": "X64", "ImageVersion": "20261001.1",
                    "RUNNER_TEMP": str(self.root.parent / "tmp"),
                    "CARGO_HOME": str(self.root.parent / "tmp/module-engine-cargo"),
                    "CARGO_INCREMENTAL": "0", "GITHUB_SHA": "a" * 40,
                    "RUSTUP_TOOLCHAIN": "1.99.0-" + self.host}
        self.rust = "rustc 1.99.0 (fixture)\nhost: " + self.host + "\nrelease: 1.99.0"
        self.cargo = "cargo 1.99.0 (fixture)\nrelease: 1.99.0"
        self.tracked = ["Cargo.toml", "Cargo.lock", "scripts/linux-toolchain.json", "engine-sdk.json"]

    def admit(self, **kwargs):
        args = {"root": self.root, "env": self.env, "rustc": self.rust,
                "cargo": self.cargo, "tracked": self.tracked, "tools": ["/tools/rustc", "/tools/cargo"]}
        args.update(kwargs)
        return ci_cache.decision(**args)

    def setup_python(self, windows=False, missing_lib=False):
        cache = self.root.parent / "toolcache"
        base = cache / "Python/3.12.13/x64"
        executable = base / ("python.exe" if windows else "bin/python")
        executable.parent.mkdir(parents=True, exist_ok=True)
        executable.write_bytes(b"Python executable fixture")
        self.env.update(pythonLocation=str(base), RUNNER_TOOL_CACHE=str(cache))
        if windows:
            (base / "python312.dll").write_bytes(b"Python DLL fixture")
            host = "x86_64-pc-windows-msvc"
            (self.root / "toolchain-manifest.json").write_text(json.dumps({"rust": {"version": "1.99.0", "target": host}}))
            self.env.update(RUNNER_OS="Windows", RUSTUP_TOOLCHAIN="1.99.0-" + host)
            self.rust = self.rust.replace(self.host, host)
        if not missing_lib:
            pc = base / "lib/pkgconfig"
            pc.mkdir(parents=True)
            (pc / "python-3.12.pc").write_bytes(b"Name: Python fixture")
            (base / "lib/libpython3.12.so.1.0").write_bytes(b"Python shared library fixture")
            self.env["PKG_CONFIG_PATH"] = str(pc)
            if not windows:
                self.env["LD_LIBRARY_PATH"] = str(base / "lib")
        return base, {"executable": str(executable), "version": "3.12.13"}

    def test_known_linux_python_context_admitted_immutable_and_content_sensitive(self):
        base, runtime = self.setup_python()
        before = self.env.copy()
        first = self.admit(runtime=runtime)
        self.assertEqual(first["enabled"], "true")
        self.assertEqual(first, self.admit(runtime=runtime))
        for path in [Path(runtime["executable"]), base / "lib/libpython3.12.so.1.0",
                     base / "lib/pkgconfig/python-3.12.pc", base / "lib/new-library.so",
                     base / "lib/pkgconfig/new.pc"]:
            old = self.admit(runtime=runtime)["restore_prefix"]
            path.write_bytes(b"changed fixture content")
            result = self.admit(runtime=runtime)
            self.assertTrue(result["enabled"] == "false" or result["restore_prefix"] != old)
        self.assertEqual(before, self.env)
        for changed in [dict(runtime, version="3.12.14"), dict(runtime, executable=str(base / "missing"))]:
            self.assertEqual(self.admit(runtime=changed)["enabled"], "false")
        relocated = dict(self.env, pythonLocation=str(base.parent / "other"))
        self.assertEqual(self.admit(runtime=runtime, env=relocated)["enabled"], "false")

    def test_known_windows_python_with_absent_lib_then_pc_inputs_accounted(self):
        base, runtime = self.setup_python(windows=True, missing_lib=True)
        first = self.admit(runtime=runtime)
        self.assertEqual(first["enabled"], "true")
        pc = base / "lib/pkgconfig"
        pc.mkdir(parents=True)
        (pc / "python.pc").write_bytes(b"known PC fixture")
        self.env["PKG_CONFIG_PATH"] = str(pc)
        second = self.admit(runtime=runtime)
        self.assertEqual(second["enabled"], "true")
        self.assertNotEqual(first["restore_prefix"], second["restore_prefix"])
        for path in [base / "python312.dll", pc / "python.pc"]:
            old = self.admit(runtime=runtime)["restore_prefix"]
            path.write_bytes(b"changed")
            self.assertNotEqual(old, self.admit(runtime=runtime)["restore_prefix"])

    def test_windows_normal_stdlib_scan_has_distinct_bound_and_tracks_native_files(self):
        base, runtime = self.setup_python(windows=True, missing_lib=True)
        lib = base / "lib"
        lib.mkdir()
        for index in range(200):
            (lib / ("stdlib_" + str(index) + ".py")).write_bytes(b"normal stdlib fixture")
        first = self.admit(runtime=runtime)
        self.assertEqual(first["enabled"], "true")
        native = lib / "additional.dll"
        native.write_bytes(b"native fixture")
        second = self.admit(runtime=runtime)
        self.assertEqual(second["enabled"], "true")
        self.assertNotEqual(first["restore_prefix"], second["restore_prefix"])
        for index in range(200, 512):
            (lib / ("stdlib_" + str(index) + ".py")).write_bytes(b"normal stdlib fixture")
        self.assertEqual(len(list(lib.iterdir())), 513)
        self.assertEqual(self.admit(runtime=runtime)["enabled"], "false")

    def test_python_context_rejects_other_search_paths_and_pkg_config_controls(self):
        base, runtime = self.setup_python()
        for name, value in [("LD_LIBRARY_PATH", str(base / "lib") + ":"),
                            ("LD_LIBRARY_PATH", str(base / "lib") + ":/private"),
                            ("LD_LIBRARY_PATH", "/private"), ("PKG_CONFIG_PATH", "/private"),
                            ("PKG_CONFIG_PATH", str(base / "lib/pkgconfig") + ":"),
                            ("PKG_CONFIG", "/private"), ("PKG_CONFIG_LIBDIR", "/private"),
                            ("PKG_CONFIG_SYSROOT_DIR", "/private"),
                            ("PKG_CONFIG_PATH_x86_64_unknown_linux_gnu", "/private"),
                            ("TARGET_PKG_CONFIG", "/private"), ("LD_PRELOAD", "/private"),
                            ("LD_AUDIT", "/private")]:
            env = dict(self.env, **{name: value})
            before = env.copy()
            result = self.admit(env=env, runtime=runtime)
            self.assertEqual(result["enabled"], "false", name)
            self.assertEqual(env, before)
            self.assertNotIn("/private", json.dumps(result))
        (base / "lib/unknown-directory").mkdir()
        self.assertEqual(self.admit(runtime=runtime)["enabled"], "false")

    def test_missing_library_and_unknown_pc_directory_rejected(self):
        base, runtime = self.setup_python()
        library = base / "lib/libpython3.12.so.1.0"
        library.unlink()
        self.assertEqual(self.admit(runtime=runtime)["enabled"], "false")
        library.write_bytes(b"restored")
        (base / "lib/pkgconfig/nested.pc").mkdir()
        self.assertEqual(self.admit(runtime=runtime)["enabled"], "false")

    def test_python_binary_missing_unreadable_inputs_and_bounded_inventory(self):
        base, runtime = self.setup_python()
        executable = Path(runtime["executable"])
        executable.unlink()
        self.assertEqual(self.admit(runtime=runtime)["enabled"], "false")
        executable.write_bytes(b"restored executable")
        original_open = Path.open
        library = base / "lib/libpython3.12.so.1.0"

        def guarded_open(path, *args, **kwargs):
            if path == library:
                raise PermissionError("private unreadable file")
            return original_open(path, *args, **kwargs)

        with patch.object(Path, "open", guarded_open):
            result = self.admit(runtime=runtime)
            self.assertEqual(result["enabled"], "false")
            self.assertNotIn("private unreadable file", json.dumps(result))
        for index in range(129):
            (base / "lib/pkgconfig" / (str(index) + ".pc")).write_bytes(b"fixture")
        self.assertEqual(self.admit(runtime=runtime)["enabled"], "false")

    def test_pkg_config_overrides_without_known_python_context_rejected(self):
        for name in ["PKG_CONFIG_PATH", "PKG_CONFIG", "PKG_CONFIG_LIBDIR",
                     "PKG_CONFIG_SYSROOT_DIR", "HOST_PKG_CONFIG", "PKG_CONFIG_ALLOW_CROSS"]:
            self.assertEqual(self.admit(env=dict(self.env, **{name: "/foreign"}))["enabled"], "false")

    def test_python_context_symlink_escape_rejected_where_supported(self):
        base, runtime = self.setup_python()
        outside = self.root.parent / "outside-library"
        outside.write_bytes(b"outside")
        link = base / "lib/escape.so"
        try:
            link.symlink_to(outside)
        except OSError as error:
            self.skipTest("Symlink creation unavailable: " + str(error))
        self.assertEqual(self.admit(runtime=runtime)["enabled"], "false")

    def test_exact_rerun_and_revision_share_only_full_compatibility_prefix(self):
        before = copy.deepcopy(self.env)
        first = self.admit()
        self.assertEqual(first["enabled"], "true")
        self.assertEqual(first, self.admit())
        self.assertEqual(self.env, before)
        self.env["GITHUB_SHA"] = "b" * 40
        next_revision = self.admit()
        self.assertNotEqual(first["key"], next_revision["key"])
        self.assertEqual(first["restore_prefix"], next_revision["restore_prefix"])
        self.assertTrue(first["key"].startswith(first["restore_prefix"]))

    def test_lock_profiles_sdk_and_policy_changes_cannot_restore_old_context(self):
        for name, payload in [("Cargo.lock", "changed lock"),
                              ("Cargo.toml", "[profile.release]\nopt-level = 3"),
                              ("engine-sdk.json", '{"sdk":"changed"}'),
                              ("scripts/verify.py", "changed verification policy")]:
            with self.subTest(name=name):
                self.tracked.append(name)
                old = self.admit()["restore_prefix"]
                (self.root / name).write_text(payload)
                self.assertNotEqual(old, self.admit()["restore_prefix"])

    def test_toolchain_host_version_and_cargo_identity_fail_closed(self):
        for overrides in [{"rustc": self.rust.replace(self.host, "aarch64-unknown-linux-gnu")},
                          {"rustc": self.rust.replace("release: 1.99.0", "release: 1.98.0")},
                          {"cargo": "cargo 1.98.0 (fixture)"}, {"tools": [None, "/cargo"]}]:
            self.assertEqual(self.admit(**overrides)["enabled"], "false")
        original = self.admit()["restore_prefix"]
        self.assertNotEqual(original, self.admit(cargo=self.cargo + "\ncommit-hash: changed")["restore_prefix"])

    def test_environment_overrides_disable_without_mutating_or_printing_values(self):
        for name in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC", "RUSTDOC",
                     "CARGO_BUILD_TARGET", "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
                     "CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_TARGET_DIR", "CARGO", "CC", "LD_PRELOAD"]:
            env = dict(self.env, **{name: "private-override"})
            before = env.copy()
            result = self.admit(env=env)
            self.assertEqual(result["enabled"], "false", name)
            self.assertNotIn("private-override", json.dumps(result))
            self.assertEqual(env, before)

    def test_config_in_checkout_ancestor_and_cargo_home_disables(self):
        for directory in [self.root / ".cargo", self.root.parent / ".cargo", Path(self.env["CARGO_HOME"])]:
            for name in ("config", "config.toml"):
                directory.mkdir(parents=True, exist_ok=True)
                config = directory / name
                config.write_text("[build]\nrustflags = ['--cfg=private']")
                self.assertEqual(self.admit()["enabled"], "false")
                config.unlink()

    def test_unknown_runner_and_image_disable_and_known_image_change_isolated(self):
        for name, value in [("RUNNER_OS", "unknown"), ("RUNNER_ARCH", "ARM64"),
                            ("ImageVersion", ""), ("GITHUB_SHA", "bad\noutput=1")]:
            self.assertEqual(self.admit(env=dict(self.env, **{name: value}))["enabled"], "false")
        self.assertNotEqual(self.admit()["restore_prefix"],
                            self.admit(env=dict(self.env, ImageVersion="20261002.1"))["restore_prefix"])

    def test_windows_native_host_is_admitted_only_with_windows_pin(self):
        host = "x86_64-pc-windows-msvc"
        (self.root / "toolchain-manifest.json").write_text(json.dumps({"rust": {"version": "1.99.0", "target": host}}))
        env = dict(self.env, RUNNER_OS="Windows", RUSTUP_TOOLCHAIN="1.99.0-" + host)
        result = self.admit(env=env, rustc=self.rust.replace(self.host, host))
        self.assertEqual(result["native_host"], host)
        self.assertEqual(result["enabled"], "true")


if __name__ == "__main__":
    unittest.main()
