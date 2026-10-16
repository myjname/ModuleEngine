"""CI-only explicit restoration of the existing Linux DXC pin into a fresh path."""
import hashlib
import json
from pathlib import Path
import tarfile
import urllib.request


def sha256(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def restore(root, download=urllib.request.urlretrieve):
    pin = json.loads((root / "scripts/linux-toolchain.json").read_text(encoding="utf-8"))["dxc"]
    destination = root / ".tools/ci-dxc"
    destination.mkdir(parents=True, exist_ok=False)
    archive = destination / "dxc.tar.gz"
    download(pin["url"], archive)
    if sha256(archive) != pin["sha256"]:
        raise ValueError("Pinned Linux DXC archive checksum mismatch")
    # Python 3.12 data filter rejects absolute/traversing paths and unsafe links.
    with tarfile.open(archive, "r:gz") as stream:
        stream.extractall(destination, filter="data")
    for relative, expected in pin["files"].items():
        path = destination / relative.removeprefix("tools/dxc/")
        if sha256(path) != expected:
            raise ValueError("Pinned DXC component checksum mismatch: " + relative)
    print("Verified Linux DXC archive and every pinned component: " + str(destination))


def main():
    restore(Path(__file__).resolve().parent.parent)


if __name__ == "__main__":
    main()
