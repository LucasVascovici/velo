#!/usr/bin/env python
"""Build the sandbox for the velo team simulation.

    python sim/team/setup.py <SIMDIR> [--no-build] [--personas a,b,c]

Creates, under SIMDIR:
    bin/real/velo[.exe]      private copy of the freshly built binary
    bin/<name>/velo          per-persona wrapper that sets VELO_AUTHOR_NAME/EMAIL
    remote/                  the shared repository (seeded toy project "shop")
    clones/<name>/           one clone per persona (heidi clones over HTTP)
    server.pid               the serve-http process serving remote/ (stop with teardown.py)
Prints one JSON object describing the layout.
"""
import json, os, shutil, subprocess, sys, time, stat
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PERSONAS = ["alice", "bob", "carol", "dan", "erin", "frank", "grace", "heidi"]
PORT = 8417
EXE = ".exe" if os.name == "nt" else ""


def run(cmd, cwd=None, env=None, check=True):
    r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise SystemExit(f"FAILED {cmd}\n{r.stdout}\n{r.stderr}")
    return r


SEED = {
    "README.md": "# shop\n\nA toy inventory and ordering library used to exercise velo.\n\n## Modules\n\n- inventory\n- pricing\n- orders\n- reports\n",
    "CHANGELOG.md": "# Changelog\n\n## Unreleased\n\n",
    "shop/__init__.py": '"""shop: toy library."""\nfrom .registry import REGISTRY\n\n__version__ = "0.1.0"\n',
    "shop/config.py": 'TAX_RATE = 0.08\nCURRENCY = "EUR"\nMAX_ITEMS_PER_ORDER = 50\nLOW_STOCK_THRESHOLD = 5\nDEFAULT_WAREHOUSE = "main"\n',
    "shop/registry.py": '"""Feature registry: every module adds one line here."""\nREGISTRY = {}\n\n\ndef register(name, fn):\n    REGISTRY[name] = fn\n    return fn\n\n\n# --- registered features (append below, one per line) ---\n',
    "shop/inventory.py": 'class Inventory:\n    def __init__(self):\n        self._stock = {}\n\n    def add(self, sku, qty):\n        self._stock[sku] = self._stock.get(sku, 0) + qty\n\n    def remove(self, sku, qty):\n        have = self._stock.get(sku, 0)\n        if qty > have:\n            raise ValueError("insufficient stock")\n        self._stock[sku] = have - qty\n\n    def qty(self, sku):\n        return self._stock.get(sku, 0)\n',
    "shop/pricing.py": 'from .config import TAX_RATE\n\n\ndef with_tax(amount):\n    return round(amount * (1 + TAX_RATE), 2)\n\n\ndef discount(amount, pct):\n    return round(amount * (1 - pct / 100), 2)\n',
    "shop/orders.py": 'from .config import MAX_ITEMS_PER_ORDER\nfrom .inventory import Inventory\n\n\nclass Order:\n    def __init__(self):\n        self.lines = []\n\n    def add(self, sku, qty, unit_price):\n        if sum(q for _, q, _ in self.lines) + qty > MAX_ITEMS_PER_ORDER:\n            raise ValueError("order too large")\n        self.lines.append((sku, qty, unit_price))\n\n    def total(self):\n        return sum(q * p for _, q, p in self.lines)\n',
    "shop/reports.py": 'def stock_report(inv):\n    return sorted(inv._stock.items())\n',
    "tests/test_basic.py": 'import unittest\nfrom shop.inventory import Inventory\nfrom shop.pricing import with_tax, discount\nfrom shop.orders import Order\n\n\nclass Basic(unittest.TestCase):\n    def test_inventory(self):\n        i = Inventory(); i.add("a", 3); i.remove("a", 1)\n        self.assertEqual(i.qty("a"), 2)\n\n    def test_pricing(self):\n        self.assertEqual(with_tax(100), 108.0)\n        self.assertEqual(discount(100, 10), 90.0)\n\n    def test_order(self):\n        o = Order(); o.add("a", 2, 5.0)\n        self.assertEqual(o.total(), 10.0)\n\n\nif __name__ == "__main__":\n    unittest.main()\n',
    "docs/guide.md": "# Guide\n\nLine one of the guide.\nLine two of the guide.\nLine three of the guide.\nLine four of the guide.\nLine five of the guide.\n",
    "data/products.json": '{\n  "products": [\n    {"sku": "a", "price": 5.0},\n    {"sku": "b", "price": 7.5}\n  ]\n}\n',
    "NOTES.txt": "Team notes. Add yours below.\n",
}


# Per-persona wrapper: sets the author, enforces a 300 s timeout, runs with no stdin (like an
# agent without a terminal) and records every invocation (args, exit code, wall time, output)
# in <sim>/cmdlog/<name>.jsonl. The perf and UX analyses are built from that log.
WRAPPER = r'''#!/usr/bin/env python
import json, os, subprocess, sys, time
from pathlib import Path
NAME = "@NAME@"
REAL = "@REAL@"
SIM = Path(__file__).resolve().parents[2]
env = dict(os.environ, VELO_AUTHOR_NAME=NAME, VELO_AUTHOR_EMAIL=NAME + "@team.test")
t0 = time.perf_counter()
try:
    p = subprocess.run([REAL, *sys.argv[1:]], env=env, capture_output=True, timeout=300,
                       stdin=subprocess.DEVNULL)
    rc, out, err = p.returncode, p.stdout, p.stderr
except subprocess.TimeoutExpired as e:
    rc, out = 124, e.stdout or b""
    err = (e.stderr or b"") + b"\n[velo-sim] killed after 300 s (hang?)\n"
ms = (time.perf_counter() - t0) * 1000
sys.stdout.buffer.write(out); sys.stdout.flush()
sys.stderr.buffer.write(err); sys.stderr.flush()
text = (out + err).decode("utf-8", "replace")
if len(text) > 1200:
    text = text[:800] + "\n[...%d chars cut...]\n" % (len(text) - 1000) + text[-200:]
rec = {"ts": time.time(), "agent": NAME, "cwd": os.getcwd().replace(chr(92), "/"), "args": sys.argv[1:],
       "rc": rc, "ms": round(ms, 1), "out": text}
try:
    with open(SIM / "cmdlog" / (NAME + ".jsonl"), "ab") as f:
        f.write((json.dumps(rec) + "\n").encode("utf-8"))
except OSError:
    pass
sys.exit(rc)
'''


def env_for(n):
    e = dict(os.environ)
    e["VELO_AUTHOR_NAME"] = n
    e["VELO_AUTHOR_EMAIL"] = f"{n}@team.test"
    return e


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if not args:
        raise SystemExit(__doc__)
    sim = Path(args[0]).resolve()
    personas = PERSONAS
    for a in sys.argv[1:]:
        if a.startswith("--personas="):
            personas = a.split("=", 1)[1].split(",")
    if sim.exists() and any(sim.iterdir()):
        raise SystemExit(f"{sim} is not empty")
    sim.mkdir(parents=True, exist_ok=True)

    if "--no-build" not in sys.argv:
        run(["cargo", "build", "--release", "-p", "velo-cli"], cwd=ROOT)
    src = ROOT / "target" / "release" / f"velo{EXE}"
    real = sim / "bin" / "real" / f"velo{EXE}"
    real.parent.mkdir(parents=True)
    shutil.copy2(src, real)

    def posix(p):
        return str(p).replace("\\", "/")

    wrappers = {}
    (sim / "cmdlog").mkdir(exist_ok=True)
    for n in personas + ["oracle"]:
        d = sim / "bin" / n
        d.mkdir(parents=True, exist_ok=True)
        w = d / "velo"
        w.write_text(WRAPPER.replace("@NAME@", n).replace("@REAL@", posix(real)), newline="\n")
        w.chmod(w.stat().st_mode | stat.S_IEXEC)
        wrappers[n] = posix(w)

    # shared remote, seeded by "oracle" (a neutral author)
    remote = sim / "remote"
    remote.mkdir()
    for rel, text in SEED.items():
        p = remote / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, newline="\n")
    run([str(real), "init"], cwd=remote, env=env_for("oracle"))
    run([str(real), "save", "Initial commit: shop skeleton"], cwd=remote, env=env_for("oracle"))

    # HTTP server over the same remote (concurrent fs + http access is deliberate)
    log = open(sim / "server.log", "w")
    flags = 0x00000008 | 0x00000200 if os.name == "nt" else 0  # DETACHED | NEW_PROCESS_GROUP
    srv = subprocess.Popen(
        [str(real), "serve-http", str(remote), "--listen", f"127.0.0.1:{PORT}"],
        stdout=log, stderr=log, creationflags=flags,
    )
    (sim / "server.pid").write_text(str(srv.pid))
    time.sleep(1.0)

    clones = {}
    (sim / "clones").mkdir()
    for n in personas:
        url = f"http://127.0.0.1:{PORT}/" if n == "heidi" else posix(remote)
        run([str(real), "clone", url, posix(sim / "clones" / n)], env=env_for(n))
        clones[n] = posix(sim / "clones" / n)

    print(json.dumps({
        "sim": posix(sim), "remote": posix(remote), "http": f"http://127.0.0.1:{PORT}/",
        "serverPid": srv.pid, "velo": wrappers, "clones": clones, "personas": personas,
    }, indent=2))


if __name__ == "__main__":
    main()
