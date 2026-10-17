"""Cache admission fixtures: no Cargo, network, native builds or Actions service."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

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
