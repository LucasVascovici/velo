"""Tests for check.py against a repository built with the `velo` on PATH."""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check  # noqa: E402

VELO = shutil.which("velo")


def make_repo(root, actor):
    env = dict(os.environ, VELO_AUTHOR_NAME=actor)
    subprocess.run([VELO, "init"], cwd=root, env=env, check=True, capture_output=True)
    with open(os.path.join(root, "notes.txt"), "w", newline="\n") as f:
        f.write("one\ntwo\n")
    subprocess.run([VELO, "save", "first"], cwd=root, env=env, check=True, capture_output=True)


@unittest.skipUnless(VELO, "velo not on PATH")
class CheckTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir, ignore_errors=True)
        make_repo(self.dir, "agent-7")

    def test_passes_for_the_right_actor(self):
        v = check.check(self.dir, VELO, "agent-7")
        self.assertTrue(v["ok"], v)
        self.assertTrue(v["fsck_ok"])
        self.assertGreaterEqual(v["history_count"], 1)
        self.assertIn("notes.txt", v["files"])

    def test_fails_for_the_wrong_actor(self):
        v = check.check(self.dir, VELO, "someone-else")
        self.assertFalse(v["ok"])
        self.assertFalse(v["blame_actor_ok"])
        self.assertTrue(v["fsck_ok"])

    def test_fails_for_a_missing_file(self):
        v = check.check(self.dir, VELO, "agent-7", ["nope.txt"])
        self.assertFalse(v["ok"])

    def test_cli_prints_json_and_exit_code(self):
        base = [sys.executable, os.path.join(HERE, "check.py"), "--repo", self.dir,
                "--velo", VELO]
        good = subprocess.run(base + ["--actor", "agent-7"], capture_output=True, text=True)
        self.assertEqual(good.returncode, 0, good.stdout)
        self.assertTrue(json.loads(good.stdout)["ok"])
        bad = subprocess.run(base + ["--actor", "nobody"], capture_output=True, text=True)
        self.assertEqual(bad.returncode, 1)
        self.assertFalse(json.loads(bad.stdout)["ok"])

    def test_empty_directory_fails(self):
        empty = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, empty, ignore_errors=True)
        self.assertFalse(check.check(empty, VELO, "agent-7")["ok"])


if __name__ == "__main__":
    unittest.main()
