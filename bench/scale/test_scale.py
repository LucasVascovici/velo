"""Unit tests for the pure helpers in scale.py. Run: python -m unittest bench/scale/test_scale.py"""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import scale  # noqa: E402


class Ratio(unittest.TestCase):
    def test_ratio_against_history0(self):
        # saves[0] is the full save and is ignored; the next 10 are 1.0 s;
        # the last 10% (10 of 100) are 1.5 s.
        saves = [50.0] + [1.0] * 89 + [1.5] * 10
        base, ratio = scale.history0_ratio(saves)
        self.assertEqual(base, 1.0)
        self.assertAlmostEqual(ratio, 1.5)

    def test_too_few_saves(self):
        self.assertEqual(scale.history0_ratio([3.0]), (None, None))


class Partial(unittest.TestCase):
    def test_incremental_json_is_valid_and_flips_partial(self):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "r.json")
            res = {"a": 1}
            scale.write_partial(out, res, ["first_save"], True)
            doc = json.load(open(out))
            self.assertTrue(doc["partial"])
            self.assertEqual(doc["completed_phases"], ["first_save"])
            res["b"] = 2
            scale.write_partial(out, res, ["first_save", "done"], False)
            doc = json.load(open(out))
            self.assertFalse(doc["partial"])
            self.assertEqual((doc["a"], doc["b"]), (1, 2))

    def test_presets_and_machine(self):
        self.assertEqual(scale.PRESETS["ratio-100k"], (100_000, 500))
        m = scale.machine_info("note")
        for key in ("os", "platform", "cpu", "logical_cores", "defender_note"):
            self.assertIn(key, m)


if __name__ == "__main__":
    unittest.main()
