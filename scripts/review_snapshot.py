#!/usr/bin/env python3
"""Bind a human/agent review to repository bytes; this does not perform a review."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys


def git(root, *args):
    result = subprocess.run(["git", *args], cwd=root, capture_output=True, timeout=30)
    if result.returncode:
        raise ValueError(result.stderr.decode("utf-8", errors="replace").strip())
    return result.stdout


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def source_digest(value):
    payload = {key: value[key] for key in ("head", "files")}
    return sha256(json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("ascii"))


def entry(root, name):
    relative = PurePosixPath(name)
    if relative.is_absolute() or ".." in relative.parts or "\\" in name or ".git" in relative.parts:
        raise ValueError("Unsafe Git inventory path: " + name)
    path = root.joinpath(*relative.parts)
    if not path.parent.resolve().is_relative_to(root):
        raise ValueError("Inventory parent escapes checkout: " + name)
    if path.is_symlink():
        kind, value = "symlink", sha256(os.fsencode(os.readlink(path)))
    elif not path.exists():
        kind, value = "missing", None
    elif path.is_file() and path.resolve().is_relative_to(root):
        checksum = hashlib.sha256()
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                checksum.update(chunk)
        kind, value = "file", checksum.hexdigest()
    else:
        raise ValueError("Unsupported inventory item: " + name)
    return {"path": name, "kind": kind, "sha256": value}


def capture(root):
    root = Path(root).resolve()
    tracked = set(filter(None, git(root, "ls-files", "--cached", "-z").decode("utf-8").split("\0")))
    untracked = set(filter(None, git(root, "ls-files", "--others", "--exclude-standard", "-z").decode("utf-8").split("\0")))
    # Review/evidence outputs must not make the snapshot invalidate itself.
    untracked = {name for name in untracked if name.split("/", 1)[0] not in ("work", "outputs")}
    value = {"schema_version": 1, "created_at": datetime.now(timezone.utc).isoformat(),
             "head": git(root, "rev-parse", "HEAD").decode("ascii").strip(),
             "files": [entry(root, name) for name in sorted(tracked | untracked)]}
    value["source_digest"] = source_digest(value)
    return value


def differences(expected, actual):
    if not isinstance(expected, dict):
        raise ValueError("Invalid or damaged review snapshot: expected an object")
    if expected.get("schema_version") != 1 or expected.get("source_digest") != source_digest(expected):
        raise ValueError("Invalid or damaged review snapshot")
    previous = {item["path"]: item for item in expected["files"]}
    current = {item["path"]: item for item in actual["files"]}
    return {"head_changed": expected["head"] != actual["head"],
            "added": sorted(current.keys() - previous.keys()),
            "removed": sorted(previous.keys() - current.keys()),
            "modified": sorted(name for name in previous.keys() & current.keys() if previous[name] != current[name])}


def main(argv=None, root=None):
    root = Path(root or Path(__file__).resolve().parents[1]).resolve()
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("capture").add_argument("--output", type=Path, required=True)
    commands.add_parser("check").add_argument("snapshot", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "capture":
            output = args.output.resolve()
            if not any(output.is_relative_to(root / name) for name in ("work", "outputs")):
                raise ValueError("Write snapshots only within checkout work/ or outputs/")
            if output.exists():
                raise ValueError("Snapshot already exists; use a new output path")
            value = capture(root)
            output.parent.mkdir(parents=True, exist_ok=True)
            with output.open("x", encoding="utf-8", newline="\n") as stream:
                json.dump(value, stream, ensure_ascii=True, indent=2)
                stream.write("\n")
            print(json.dumps({"snapshot": str(output), "head": value["head"], "source_digest": value["source_digest"]}))
            return 0
        expected = json.loads(args.snapshot.read_text(encoding="utf-8"))
        actual = capture(root)
        changed = differences(expected, actual)
        fresh = expected["source_digest"] == actual["source_digest"]
        print(json.dumps({"snapshot_matches": fresh, "source_digest": actual["source_digest"], **changed}))
        return 0 if fresh else 1
    except (OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
        print("Review snapshot error: " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
