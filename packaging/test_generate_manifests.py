import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import generate_manifests as g


class GenerateTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.assets, self.out = root / "a", root / "o"
        self.assets.mkdir()
        self.sums = {}
        for key, name in g.ASSETS.items():
            data = ("fake " + name).encode()
            (self.assets / name).write_bytes(data)
            self.sums[key] = hashlib.sha256(data).hexdigest()
        self.files = g.generate("v1.2.3", self.assets, self.out)

    def tearDown(self):
        self.tmp.cleanup()

    def url(self, key):
        return "%s/releases/download/v1.2.3/%s" % (g.REPO_URL, g.ASSETS[key])

    def test_formula(self):
        text = (self.out / "homebrew/velody.rb").read_text()
        for key in g.ASSETS:
            if key.endswith("WINDOWS"):
                continue
            self.assertIn('url "%s"' % self.url(key), text)
            self.assertIn('sha256 "%s"' % self.sums[key], text)
        self.assertIn('bin.install "velody"', text)
        self.assertIn('"--version"', text)

    def test_scoop(self):
        m = json.loads((self.out / "scoop/velody.json").read_text())
        arch = m["architecture"]["64bit"]
        self.assertEqual(m["version"], "1.2.3")
        self.assertEqual(arch["url"], self.url("X86_64_WINDOWS"))
        self.assertEqual(arch["hash"], self.sums["X86_64_WINDOWS"])
        self.assertEqual(m["bin"], "velody.exe")

    def test_winget(self):
        d = self.out / "winget/manifests/l/LucasVascovici/Velody/1.2.3"
        self.assertEqual(len(list(d.glob("*.yaml"))), 3)
        inst = (d / "LucasVascovici.Velody.installer.yaml").read_text()
        self.assertIn(self.url("X86_64_WINDOWS"), inst)
        self.assertIn(self.sums["X86_64_WINDOWS"].upper(), inst)
        self.assertIn("PortableCommandAlias: velody", inst)
        self.assertIn("ManifestVersion: " + g.WINGET_SCHEMA, inst)

    def test_no_placeholders_left(self):
        for f in self.files:
            self.assertNotIn("@@", f.read_text())

    def test_missing_asset_and_bad_tag(self):
        (self.assets / g.ASSETS["X86_64_LINUX"]).unlink()
        with self.assertRaises(SystemExit):
            g.generate("v1.2.3", self.assets, self.out)
        with self.assertRaises(SystemExit):
            g.generate("1.2.3", self.assets, self.out)


if __name__ == "__main__":
    unittest.main()
