"""Print one task's brief from a saved implement-phase plan.

    python .claude/scripts/brief.py <plan.json> <task-id>

The workflow passes agents a pointer to their brief rather than the brief
itself, so the orchestrator never has to restate a plan it already saved.
"""
import json
import sys

sys.stdout.reconfigure(encoding="utf-8")
path, task_id = sys.argv[1], sys.argv[2]
saved = json.load(open(path, encoding="utf-8"))
tasks = (saved.get("plan") or saved)["tasks"]
t = next((t for t in tasks if t["id"] == task_id), None)
if t is None:
    sys.exit(f"no task {task_id!r} in {path}")
print(f"## Task {t['id']} — {t['title']}  (ARCHITECTURE.md {t['item']})\n")
print(t["brief"])
print("\n### Files expected")
for f in t["files"]:
    print(f"- {f}")
print("\n### Acceptance criteria")
for i, c in enumerate(t["acceptance"], 1):
    print(f"{i}. {c}")
print(f"\n### Tests\n{t['tests']}")
