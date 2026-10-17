#!/usr/bin/env python3
"""Conservative hosted Cargo cache admission; never changes the build environment."""
import hashlib
from itertools import islice
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


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


def python_context(env, runtime):
    """Account only for setup-python's single known loader/pkg-config context."""
    location = env.get("pythonLocation", "")
    loader = env.get("LD_LIBRARY_PATH", "")
    pkg = env.get("PKG_CONFIG_PATH", "")
    for name, value in env.items():
        if value and ("PKG_CONFIG" in name and name != "PKG_CONFIG_PATH"):
            raise ValueError("unaccounted pkg-config environment")
    if not location:
        if loader or pkg:
            raise ValueError("loader or pkg-config path without Python context")
        return {"setup_python": "absent"}
    base = Path(location)
    cache = Path(env.get("RUNNER_TOOL_CACHE", ""))
    version = runtime["version"]
    if (not base.is_absolute() or not cache.is_absolute()
            or not re.fullmatch(r"3\.12\.[0-9]+", version)
            or base != base.resolve() or cache != cache.resolve()
            or base != cache / "Python" / version / "x64" or not base.is_dir()):
        raise ValueError("unverified setup-python location")
    executable = Path(runtime["executable"])
    expected_bin = base if env["RUNNER_OS"] == "Windows" else base / "bin"
    if (not executable.is_absolute() or executable.parent != expected_bin
            or not executable.resolve().is_relative_to(base)):
        raise ValueError("unverified Python executable")
    lib = base / "lib"
    pc = lib / "pkgconfig"
    if loader and (env["RUNNER_OS"] != "Linux" or loader != str(lib)):
        raise ValueError("unaccounted loader search path")
    if pkg and Path(pkg) != pc:
        raise ValueError("unaccounted pkg-config search path")
    # Every referenced file stays in the known installation. Bound both work
    # and memory; reject nested pkg-config directories and escaping symlinks.
    inventory = {}
    remaining = 256 * 1024 * 1024

    def record(path):
        nonlocal remaining
        resolved = path.resolve(strict=True)
        if not resolved.is_relative_to(base) or not resolved.is_file() or len(inventory) >= 128:
            raise ValueError("unsupported Python context file")
        size = resolved.stat().st_size
        if size > remaining:
            raise ValueError("oversized Python context")
        remaining -= size
        digest = hashlib.sha256()
        read = 0
        with resolved.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                read += len(chunk)
                if read > size:
                    raise ValueError("Python context file changed while reading")
                digest.update(chunk)
        if read != size:
            raise ValueError("Python context file changed while reading")
        inventory[str(path.relative_to(base))] = ["symlink" if path.is_symlink() else "file",
                                                str(resolved.relative_to(base)), digest.hexdigest()]

    record(executable)
    for directory in (lib, pc):
        if directory.is_symlink() or (directory.exists() and not directory.is_dir()):
            raise ValueError("unsupported Python context directory")
    if env["RUNNER_OS"] == "Linux":
        if not lib.is_dir():
            raise ValueError("missing Python library directory")
        entries = list(islice(lib.iterdir(), 129))
        if len(entries) > 128:
            raise ValueError("oversized Python library inventory")
        for path in sorted(entries):
            if path.is_dir():
                if path.name not in {"python3.12", "pkgconfig"} or path.is_symlink():
                    raise ValueError("unknown Python library subdirectory")
            else:
                record(path)
        if not any(path.name.startswith("libpython3.12.so") for path in entries):
            raise ValueError("missing Python runtime library")
        library_names = sorted((path.name, "directory" if path.is_dir() else "file") for path in entries)
    else:
        # Windows loads CPython DLLs beside python.exe; Lib is its stdlib and
        # must not be recursively hashed as a native loader directory.
        entries = list(islice(base.iterdir(), 129))
        if len(entries) > 128:
            raise ValueError("oversized Python runtime inventory")
        for path in sorted(entries):
            if path.suffix.lower() == ".dll":
                record(path)
        if not any(path.name.lower() == "python312.dll" for path in entries):
            raise ValueError("missing Python runtime library")
        library_names = sorted(path.name for path in entries if path.suffix.lower() == ".dll")
        if lib.exists():
            # Windows lib is case-insensitively Lib, the Python stdlib. Its
            # shallow scan needs room for ordinary .py files; only direct native
            # loadables consume the separate 128-file/256-MiB digest budget.
            direct = list(islice(lib.iterdir(), 513))
            if len(direct) > 512:
                raise ValueError("oversized Python library inventory")
            for path in sorted(direct):
                if path.suffix.lower() in {".dll", ".so", ".dylib"} or ".so." in path.name:
                    record(path)
    if pc.exists():
        entries = list(islice(pc.iterdir(), 129))
        if len(entries) > 128:
            raise ValueError("oversized pkg-config inventory")
        for path in sorted(entries):
            if path.suffix != ".pc":
                raise ValueError("unknown pkg-config input")
            record(path)
    return {"setup_python": str(base), "version": version, "executable": str(executable),
            "loader": loader, "pkg_config": pkg, "lib_exists": lib.exists(),
            "pkgconfig_exists": pc.exists(), "library_names": library_names, "files": inventory}


def decision(root, env, rustc, cargo, tracked, tools, runtime=None):
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
                            "LINK", "_LINK_", "CL", "_CL_", "LD_PRELOAD", "LD_AUDIT"}
                or name.startswith("DYLD_")
                or name.startswith(("CC_", "CXX_", "AR_", "CFLAGS_", "CXXFLAGS_"))):
            return reject("unaccounted compiler environment")
    try:
        python = python_context(env, runtime or {"executable": sys.executable,
                                                "version": ".".join(map(str, sys.version_info[:3]))})
    except (OSError, ValueError, KeyError, RuntimeError):
        return reject("unverified Python loader or pkg-config context")
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
                "python": python, "inputs": source_digest(root, tracked)}
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
