import json
import tempfile
import unittest
from pathlib import Path

import release_assets as r


class ReleaseAssetsTest(unittest.TestCase):
    def run_check(self, names):
        with tempfile.TemporaryDirectory() as tmp:
            f = Path(tmp) / "assets.json"
            f.write_text(json.dumps({"assets": [{"name": n} for n in names]}))
            return r.main(["--tag", "v1.2.3", "--assets-json", str(f)])

    def test_expected_has_every_class(self):
        names = r.expected("1.2.3")
        self.assertEqual(len(names), 5 + 5 + 3 + 3 + 4 + 3)
        self.assertIn("velody-1.2.3.cdx.json", names)
        self.assertIn("velody_core-1.2.3-cp39-abi3-win_amd64.whl", names)

    def test_complete_set_passes(self):
        self.assertEqual(self.run_check(r.expected("1.2.3")), 0)

    def test_missing_wheel_fails(self):
        names = r.expected("1.2.3")
        gone = "velody_core-1.2.3-cp39-abi3-win_amd64.whl"
        names.discard(gone)
        self.assertEqual(r.compare("1.2.3", names)[0], [gone])
        self.assertEqual(self.run_check(names), 1)

    def test_extra_file_only_warns(self):
        names = r.expected("1.2.3") | {"notes.txt"}
        self.assertEqual(r.compare("1.2.3", names), ([], ["notes.txt"]))
        self.assertEqual(self.run_check(names), 0)


if __name__ == "__main__":
    unittest.main()
