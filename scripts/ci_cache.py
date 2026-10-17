#!/usr/bin/env python3
"""Conservative hosted Cargo cache admission; never changes the build environment."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess


HOSTS = {"Windows": "x86_64-pc-windows-msvc", "Linux": "x86_64-unknown-linux-gnu"}
SAFE_CARGO = {"CARGO_HOME", "CARGO_INCREMENTAL", "CARGO_TERM_COLOR"}
POLICIES = {"Cargo.lock", "Cargo.toml", "engine-sdk.json", "toolchain-manifest.json", "rust-toolchain",
            "rust-toolchain.toml", "verify.ps1", "verify.sh", ".github/workflows/ci.yml",
            "scripts/verify.py", "scripts/verification.json", "scripts/ci_plan.py",
            "scripts/ci_linux_dxc.py", "scripts/linux-toolchain.json", "scripts/test_ci.py"}


def command(root, argv):
    return subprocess.run(argv, cwd=root, check=True, capture_output=True,
                          text=True, timeout=30).stdout.strip()


def source_digest(root, tracked):
    names = set(POLICIES) | {"scripts/ci_cache.py", "scripts/test_ci_cache.py"}
    for name in tracked:
        if (name.endswith("/Cargo.toml")
                or name.startswith("crates/engine-core/")
                or name.startswith("crates/runtime-script/")
                or name.endswith(("/.cargo/config", "/.cargo/config.toml"))):
            names.add(name)
    digest = hashlib.sha256()
    for name in sorted(names):
        path = root / name
        if path.is_symlink():
            raise ValueError("symlinked compatibility input")
        payload = path.read_bytes() if path.is_file() else b"<absent>"
        for part in (name.encode(), payload):
            digest.update(len(part).to_bytes(8, "big"))
            digest.update(part)
    return digest.hexdigest()


def decision(root, env, rustc, cargo, tracked, tools):
    """Pure admission policy apart from reading compatibility files."""
    root = Path(root).resolve()
    denied = {"enabled": "false", "key": "", "restore_prefix": "", "native_host": ""}

    def reject(reason):
        return dict(denied, reason=reason)

    host = HOSTS.get(env.get("RUNNER_OS"))
    image = env.get("ImageVersion", "")
    sha = env.get("GITHUB_SHA", "")
    if (not host or env.get("RUNNER_ARCH") != "X64"
            or not re.fullmatch(r"[A-Za-z0-9._-]{1,100}", image)
            or not re.fullmatch(r"[0-9a-f]{40}", sha)):
        return reject("unsupported runner identity")
    cargo_home = Path(env.get("CARGO_HOME", ""))
    if not cargo_home.is_absolute() or env.get("CARGO_INCREMENTAL") != "0":
        return reject("unsupported Cargo home or incremental setting")
    expected_home = Path(env.get("RUNNER_TEMP", "")) / "module-engine-cargo"
    if cargo_home.resolve() != expected_home.resolve():
        return reject("unexpected Cargo home")
    for name, value in env.items():
        if not value:
            continue
        if ((name.startswith("CARGO_") and name not in SAFE_CARGO)
                or (name.startswith("RUST") and name not in {"RUSTUP_HOME", "RUSTUP_TOOLCHAIN"})
                or name in {"CARGO", "CC", "CXX", "AR", "CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS",
                            "LINK", "_LINK_", "CL", "_CL_", "LD_PRELOAD", "LD_AUDIT", "LD_LIBRARY_PATH"}
                or name.startswith("DYLD_")
                or name.startswith(("CC_", "CXX_", "AR_", "CFLAGS_", "CXXFLAGS_"))):
            return reject("unaccounted compiler environment")
    # Cargo searches the current directory's ancestors and CARGO_HOME. Do not
    # invent support for config (including includes, wrappers or linker overrides).
    for directory in (root, *root.parents):
        if any((directory / ".cargo" / name).exists() or (directory / ".cargo" / name).is_symlink()
               for name in ("config", "config.toml")):
            return reject("effective Cargo configuration")
    if any((cargo_home / name).exists() or (cargo_home / name).is_symlink() for name in ("config", "config.toml")):
        return reject("effective Cargo home configuration")
    pin_file = "toolchain-manifest.json" if env["RUNNER_OS"] == "Windows" else "scripts/linux-toolchain.json"
    pin = json.loads((root / pin_file).read_text(encoding="utf-8"))["rust"]
    releases = re.findall(r"^release: (\S+)$", rustc, re.MULTILINE)
    hosts = re.findall(r"^host: (\S+)$", rustc, re.MULTILINE)
    if (hosts != [host] or pin["target"] != host or releases != [pin["version"]]
            or env.get("RUSTUP_TOOLCHAIN") != pin["version"] + "-" + host
            or not re.match(r"^cargo " + re.escape(pin["version"]) + r"(?:\s|$)", cargo)
            or not all(tools)):
        return reject("unverified pinned native toolchain")
    identity = {"os": env["RUNNER_OS"], "arch": env["RUNNER_ARCH"], "image": image,
                "host": host, "rustc": rustc, "cargo": cargo, "tools": tools,
                "workspace": str(root), "cargo_home": str(cargo_home.resolve()),
                "rustup_home": env.get("RUSTUP_HOME", ""),
                "inputs": source_digest(root, tracked)}
    compatibility = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    prefix = "me-cargo-v1-" + env["RUNNER_OS"] + "-X64-" + host + "-" + compatibility + "-"
    return {"enabled": "true", "key": prefix + sha, "restore_prefix": prefix,
            "native_host": host, "reason": "compatible pinned hosted context"}


def main():
    root = Path(__file__).resolve().parent.parent
    try:
        tracked = command(root, ["git", "ls-files", "-z"]).split("\0")
        tools = [shutil.which(name) for name in ("rustc", "cargo")]
        result = decision(root, os.environ, command(root, ["rustc", "-vV"]),
                          command(root, ["cargo", "-vV"]), tracked, tools)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        # Cache admission is optional. Existing mandatory verification still
        # diagnoses the original environment without any rewriting or fallback.
        result = {"enabled": "false", "key": "", "restore_prefix": "", "native_host": "",
                  "reason": "cache compatibility could not be established"}
    print(json.dumps(result, sort_keys=True))
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as stream:
            for name in ("enabled", "key", "restore_prefix", "native_host"):
                value = result[name]
                if not re.fullmatch(r"[A-Za-z0-9._-]*", value):
                    raise ValueError("unsafe cache output")
                stream.write(name + "=" + value + "\n")


if __name__ == "__main__":
    main()
