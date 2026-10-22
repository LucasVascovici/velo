"""Tests for the cold-start kit. Run: python -m unittest discover -s bench/coldstart -p 'test_*.py'"""
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import audit  # noqa: E402
import prepare  # noqa: E402
import rank  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))


class AuditTests(unittest.TestCase):
    def test_architecture_read_is_invalid(self):
        inv, _ = audit.audit("$ cat /repo/ARCHITECTURE.md", "/tmp/sb")
        self.assertTrue(inv)

    def test_source_path_is_invalid(self):
        inv, _ = audit.audit("Read crates/velo-core/src/lib.rs", "/tmp/sb")
        self.assertTrue(inv)

    def test_jsonl_is_scanned(self):
        inv, _ = audit.audit(audit.transcript_text('{"cmd": "cat CLAUDE.md"}'), "/tmp/sb")
        self.assertTrue(inv)

    def test_clean_transcript_passes_and_outside_path_warns(self):
        text = "wrote /tmp/sb/work/a.txt and ran /usr/bin/env"
        inv, warn = audit.audit(text, "/tmp/sb")
        self.assertEqual(inv, [])
        self.assertEqual(len(warn), 1)

    def test_exit_codes(self):
        with tempfile.TemporaryDirectory() as d:
            bad = os.path.join(d, "bad.txt")
            good = os.path.join(d, "good.txt")
            with open(bad, "w") as f:
                f.write("open ARCHITECTURE.md\n")
            with open(good, "w") as f:
                f.write("ls /usr/bin\n")
            script = os.path.join(HERE, "audit.py")
            self.assertEqual(subprocess.run([sys.executable, script, bad, "--sandbox", d], capture_output=True).returncode, 1)
            self.assertEqual(subprocess.run([sys.executable, script, good, "--sandbox", d], capture_output=True).returncode, 0)


class PrepareTests(unittest.TestCase):
    def test_refuses_root_inside_git_work_tree(self):
        with tempfile.TemporaryDirectory() as d:
            subprocess.run(["git", "init", "-q", d], check=True)
            root = os.path.join(d, "sub")
            os.makedirs(root)
            self.assertTrue(prepare.inside_git_tree(root))
            rc = prepare.main(["--tag", "v0.0.0", "--wedge", "rust", "--run", "1", "--root", root])
            self.assertEqual(rc, 2)
            self.assertFalse(os.path.exists(os.path.join(root, "rust-run1")))

    def test_term_counter(self):
        c = prepare.term_counts("save_changes save_changes velo report")
        self.assertEqual(c, {"save_changes": 2, "velo report": 1, "velo sync": 0})

    def test_card_parsing(self):
        card = os.path.join(HERE, "tasks", "python-agent-checkpointing.md")
        sentence, line, actor, _ = prepare.parse_card(card)
        self.assertTrue(sentence.startswith("An agent that edits files"))
        self.assertEqual(actor, "coldstart-py")
        self.assertNotIn("Give", line)
        _, _, actor, file = prepare.parse_card(os.path.join(HERE, "tasks", "node-config-registry.md"))
        self.assertEqual((actor, file), ("coldstart-node", "configs/service.yaml"))

    def test_checksum_mismatch_stops(self):
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, "a.whl"), "wb") as f:
                f.write(b"data")
            with open(os.path.join(d, "SHA256SUMS-x.txt"), "w") as f:
                f.write("0" * 64 + "  a.whl\n")
            with self.assertRaises(SystemExit):
                prepare.verify(d, ["a.whl"])
            with open(os.path.join(d, "SHA256SUMS-x.txt"), "w") as f:
                f.write(prepare.sha256(os.path.join(d, "a.whl")) + "  a.whl\n")
            prepare.verify(d, ["a.whl"])


class RankTests(unittest.TestCase):
    LOG = """
| Run | Wedge | Minute | Kind | Quote | Defect | Proposed fix |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | python | 3 | guess | q | no-save-example | x |
| 1 | python | 5 | error | q | no-save-example | x |
| 2 | python | 4 | guess | q | no-save-example | x |
| 1 | node | 4 | error | q | sync-missing | x |
"""

    def test_counts_distinct_runs_not_rows(self):
        ranked = rank.rank([self.LOG])
        self.assertEqual(ranked[0][0], "no-save-example")
        self.assertEqual(len(ranked[0][1]["runs"]), 2)
        self.assertEqual(len(ranked[1][1]["runs"]), 1)
        self.assertIn("| no-save-example | 2 |", rank.render(ranked))


if __name__ == "__main__":
    unittest.main()
