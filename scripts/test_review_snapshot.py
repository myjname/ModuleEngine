"""Repository-byte binding only; none of these tests reviews product changes."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import review_snapshot as review


class ReviewSnapshotTests(unittest.TestCase):
    def setUp(self):
        fixtures = Path.cwd() / "work" / "review-snapshot-tests"
        fixtures.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=fixtures)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.tracked, self.untracked, self.head = {"tracked.rs"}, {"new.rs"}, "a" * 40
        for name in self.tracked | self.untracked:
            (self.root / name).write_text("initial", encoding="utf-8")
        self.mock = patch.object(review, "git", self.git)
        self.mock.start()
        self.addCleanup(self.mock.stop)

    def git(self, root, *args):
        self.assertEqual(root, self.root)
        if args == ("ls-files", "--cached", "-z"):
            return ("\0".join(sorted(self.tracked)) + "\0").encode()
        if args == ("ls-files", "--others", "--exclude-standard", "-z"):
            return ("\0".join(sorted(self.untracked)) + "\0").encode()
        self.assertEqual(args, ("rev-parse", "HEAD"))
        return self.head.encode()

    def run_cli(self, *args):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return review.main(args, self.root)

    def test_same_bytes_have_same_digest_and_output_does_not_invalidate(self):
        target = self.root / "outputs" / "review" / "source.json"
        self.assertEqual(self.run_cli("capture", "--output", str(target)), 0)
        self.untracked.update({"outputs/review/source.json", "work/notes.md"})
        self.assertEqual(self.run_cli("check", str(target)), 0)
        self.assertEqual(self.run_cli("capture", "--output", str(target)), 2)

    def test_tracked_and_untracked_changes_reject_stale_snapshot(self):
        for name in ("tracked.rs", "new.rs"):
            with self.subTest(name=name):
                expected = review.capture(self.root)
                (self.root / name).write_text("modified", encoding="utf-8")
                current = review.capture(self.root)
                self.assertNotEqual(expected["source_digest"], current["source_digest"])
                self.assertEqual(review.differences(expected, current)["modified"], [name])
                (self.root / name).write_text("initial", encoding="utf-8")

    def test_added_deleted_and_new_head_reject_stale_snapshot(self):
        expected = review.capture(self.root)
        self.untracked.add("another.rs")
        (self.root / "another.rs").write_text("new", encoding="utf-8")
        (self.root / "tracked.rs").unlink()
        self.head = "b" * 40
        changed = review.differences(expected, review.capture(self.root))
        self.assertEqual(changed, {"head_changed": True, "added": ["another.rs"], "removed": [], "modified": ["tracked.rs"]})

    def test_cli_rejects_change_and_damaged_snapshot(self):
        target = self.root / "outputs" / "source.json"
        self.assertEqual(self.run_cli("capture", "--output", str(target)), 0)
        (self.root / "new.rs").write_text("changed", encoding="utf-8")
        self.assertEqual(self.run_cli("check", str(target)), 1)
        value = json.loads(target.read_text(encoding="utf-8"))
        value["files"][0]["sha256"] = "0" * 64
        target.write_text(json.dumps(value), encoding="utf-8")
        self.assertEqual(self.run_cli("check", str(target)), 2)

    def test_cli_rejects_non_object_snapshot_as_an_error(self):
        target = self.root / "outputs" / "invalid.json"
        target.parent.mkdir()
        for invalid in ([], None, 1, "snapshot"):
            with self.subTest(value=invalid):
                target.write_text(json.dumps(invalid), encoding="utf-8")
                self.assertEqual(self.run_cli("check", str(target)), 2)

    def test_does_not_follow_symlink_payload(self):
        link = self.root / "outside-link"
        try:
            link.symlink_to(self.root.parent / "outside-content")
        except OSError as error:
            self.skipTest("OS does not permit test symlink: " + str(error))
        self.untracked.add(link.name)
        value = next(item for item in review.capture(self.root)["files"] if item["path"] == link.name)
        self.assertEqual(value["kind"], "symlink")
        self.assertEqual(value["sha256"], review.sha256(os.fsencode(os.readlink(link))))

    def test_rejects_path_escape_and_output_outside_evidence(self):
        self.untracked.add("../outside.rs")
        with self.assertRaisesRegex(ValueError, "Unsafe"):
            review.capture(self.root)
        self.assertEqual(self.run_cli("capture", "--output", str(self.root / "source.json")), 2)

    def test_git_failure_is_an_error_not_a_matching_snapshot(self):
        with patch.object(review, "git", side_effect=ValueError("Git unavailable")):
            self.assertEqual(self.run_cli("capture", "--output", str(self.root / "outputs" / "source.json")), 2)


if __name__ == "__main__":
    unittest.main()
