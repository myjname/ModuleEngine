"""Small CI path selection and fail-closed aggregate; no GitHub API or dependencies."""
import argparse
import json
import os
from pathlib import PurePosixPath
import re
import subprocess


def select(paths):
    # Only documentation-only changes omit the additional native/package routes.
    # Unknown/new source paths fail toward more coverage, not a missed subsystem.
    relevant = any(not (path.startswith("docs/") or
                        PurePosixPath(path).suffix.lower() == ".md" or
                        path in {"LICENSE", "LICENSE-MIT", ".gitignore"}) for path in paths)
    return {"native": relevant, "package": relevant, "paths": paths}


def changed_paths(base, head="HEAD"):
    if not base or base == "0" * 40:
        return ["<initial-or-manual-run>"]
    if not re.fullmatch(r"[a-fA-F0-9]{40}", base):
        raise ValueError("Base must be a full Git SHA")
    result = subprocess.run(["git", "diff", "--no-ext-diff", "--no-renames", "--name-only", "-z",
                             base, head, "--"], check=True, capture_output=True)
    return sorted(path.decode("utf-8", errors="surrogateescape")
                  for path in result.stdout.split(b"\0") if path)


def check_gate(plan_result, checks_result, selected):
    if plan_result != "success" or checks_result != "success":
        raise ValueError("CI requires successful planning and both OS jobs; failure, skip or cancellation is not acceptance")
    if set(selected) != {"native", "package"} or any(value not in {"true", "false"} for value in selected.values()):
        raise ValueError("Missing or invalid route selection")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="")
    parser.add_argument("--gate", action="store_true")
    args = parser.parse_args()
    if args.gate:
        selected = {key: os.environ.get("CI_" + key.upper(), "") for key in ("native", "package")}
        check_gate(os.environ.get("CI_PLAN_RESULT"), os.environ.get("CI_CHECKS_RESULT"), selected)
        print("CI required passed; route selection: " + json.dumps(selected))
        return
    selected = select(changed_paths(args.base))
    print(json.dumps(selected, ensure_ascii=True, indent=2))
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as stream:
            for key in ("native", "package"):
                stream.write(key + "=" + str(selected[key]).lower() + "\n")


if __name__ == "__main__":
    main()
