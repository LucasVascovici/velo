#!/usr/bin/env python3
"""Rank friction-log defects by how many distinct runs hit them.

    rank.py LOG.md [LOG.md ...]

Reads the event table (columns Run, Wedge, Kind, Defect, ...) of each log and
prints a markdown table to paste into the log's `## Ranked` section. A run is
counted once per defect however many rows it has.
"""
import sys


def _cells(line):
    return [c.strip() for c in line.strip().strip("|").split("|")]


def parse(text):
    """Yield (run, wedge, kind, defect) from every table with Run and Defect columns."""
    header = None
    for line in text.splitlines():
        if not line.lstrip().startswith("|"):
            header = None
            continue
        cells = _cells(line)
        low = [c.lower() for c in cells]
        if "run" in low and "defect" in low:
            header = low
            continue
        if header is None or set("".join(cells)) <= set("-: "):
            continue
        row = dict(zip(header, cells))
        defect = row.get("defect", "")
        if not defect:
            continue
        yield (row.get("run", ""), row.get("wedge", ""), row.get("kind", ""), defect)


def rank(texts):
    table = {}
    for src, text in enumerate(texts):
        for run, wedge, kind, defect in parse(text):
            e = table.setdefault(defect, {"runs": set(), "kinds": set(), "wedges": set()})
            e["runs"].add((src, wedge, run))
            if kind:
                e["kinds"].add(kind)
            if wedge:
                e["wedges"].add(wedge)
    return sorted(table.items(), key=lambda kv: (-len(kv[1]["runs"]), kv[0]))


def render(ranked):
    out = ["| Defect | Runs hit | Kinds | Wedges |", "| --- | --- | --- | --- |"]
    for d, e in ranked:
        out.append(
            "| %s | %d | %s | %s |"
            % (d, len(e["runs"]), ", ".join(sorted(e["kinds"])), ", ".join(sorted(e["wedges"])))
        )
    return "\n".join(out)


def main(argv=None):
    paths = argv if argv is not None else sys.argv[1:]
    if not paths:
        print("usage: rank.py LOG.md [LOG.md ...]", file=sys.stderr)
        return 2
    texts = []
    for p in paths:
        with open(p, encoding="utf-8") as f:
            texts.append(f.read())
    print(render(rank(texts)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
