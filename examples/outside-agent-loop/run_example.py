"""Wire velo into mini-swe-agent's DefaultAgent loop.

The loop (minisweagent.agents.default.DefaultAgent) is third-party code and is
used unmodified, installed from requirements.txt. This file subclasses it to
checkpoint the workspace around every tool call, then restores an earlier step.

No LLM and no network: the model is mini-swe-agent's own DeterministicModel,
which replays a fixed list of assistant messages.
"""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import velo
from minisweagent.agents.default import DefaultAgent
from minisweagent.environments.local import LocalEnvironment
from minisweagent.models.test_models import DeterministicModel, make_output

RUN = "run-1"
MODEL = "deterministic"
BRANCH = "main"


def snapshot_dir(root: Path) -> dict[str, bytes]:
    """Every regular file under root, by posix path."""
    return {
        p.relative_to(root).as_posix(): p.read_bytes()
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


class VeloAgent(DefaultAgent):
    """DefaultAgent that saves the workspace after every tool call."""

    def __init__(self, *args, repo: "velo.Repo", workspace: Path, **kwargs):
        super().__init__(*args, **kwargs)
        self.repo = repo
        self.workspace = workspace
        self.step_no = 0
        self.mirror = snapshot_dir(workspace)  # what the last snapshot holds
        self.tip = None
        self.steps: list[str] = []  # snapshot id per step, step 0 = start
        self.tip = self._save({}, "step 0: start", 0, "")

    def _save(self, changes, message, step, command):
        saved = self.repo.save_changes(
            branch=BRANCH,
            message=message,
            changes=changes,
            parent=self.tip,
            author=velo.Author(f"agent-{RUN}"),
            meta={"agent": {"run": RUN, "step": str(step), "model": MODEL,
                            "command": command[:200]}},
        )
        self.steps.append(saved.snapshot)
        return saved.snapshot

    def execute_actions(self, message: dict) -> list[dict]:
        self.step_no += 1
        commands = [a.get("command", "") for a in message["extra"]["actions"]]
        try:
            return super().execute_actions(message)
        finally:
            # finally: the submit command raises out of execute_actions, and
            # the step before it must still be checkpointed.
            now = snapshot_dir(self.workspace)
            changes = {p: b for p, b in now.items() if self.mirror.get(p) != b}
            changes |= {p: None for p in self.mirror if p not in now}
            self.mirror = now
            if changes:
                label = commands[0].replace(sys.executable, "python")
                self.tip = self._save(changes, f"step {self.step_no}: {label[:60]}",
                                      self.step_no, label)


def py(code: str) -> str:
    """A shell command that runs code with this interpreter, on cmd and sh.

    The code avoids backslashes: they do not survive both shells, so newlines
    are spelled chr(10).
    """
    return f'"{sys.executable}" -c "{code}"'


def write(path: str, *lines: str) -> str:
    body = "+chr(10)+".join(repr(line) for line in lines) + "+chr(10)"
    return py(f"open('{path}','w').write({body})")


SCRIPT = [
    ("Create the module.", write("calc.py", "def add(a, b):", "    return a + b")),
    ("Add a test.", write("test_calc.py", "from calc import add", "assert add(1, 2) == 3")),
    ("Break add (a bad edit).", write("calc.py", "def add(a, b):", "    return a - b")),
    ("Run the test.", py("import subprocess,sys;sys.exit(subprocess.call([sys.executable,'test_calc.py']))")),
    ("Done.", "echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT"),
]


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="velo-outside-"))
    workspace = tmp / "workspace"
    workspace.mkdir()
    repo = velo.Repo.init(tmp / "store")

    model = DeterministicModel(
        outputs=[make_output(text, [{"command": cmd}]) for text, cmd in SCRIPT]
    )
    agent = VeloAgent(
        model, LocalEnvironment(cwd=str(workspace)), repo=repo, workspace=workspace,
        system_template="You are a coding agent.",
        instance_template="Task: {{task}}",
        cost_limit=0,  # the default 3.0 stops a DeterministicModel at cost 1.0/call
    )
    result = agent.run("write add() and test it")
    print("agent exit:", result.get("exit_status"))

    print("\ncheckpoints (newest first):")
    for e in repo.history(from_=agent.tip):
        m = repo.snapshot_meta(e.id)["agent"]
        print(f"  {e.id[:12]}  step={m['step']}  {e.message}")
    assert len(repo.history(from_=agent.tip)) == len(agent.steps)

    # Restore: the bad edit is step 3; go back to step 2.
    target = agent.steps[2]
    print("\nfiles now:", {k: v.decode() for k, v in snapshot_dir(workspace).items()})
    for f in workspace.iterdir():
        f.unlink() if f.is_file() else shutil.rmtree(f)
    repo.materialize(target, workspace)
    restored = snapshot_dir(workspace)
    print("restored to step 2:", {k: v.decode() for k, v in restored.items()})
    # velo stores text with LF endings, so a Windows CRLF file comes back as LF.
    assert restored["calc.py"] == b"def add(a, b):\n    return a + b\n"
    assert sorted(restored) == ["calc.py", "test_calc.py"]
    test = subprocess.run([sys.executable, "test_calc.py"], cwd=workspace)
    assert test.returncode == 0, "the restored step-2 tree passes its own test"

    # Carry on from the restored step on its own branch, as the cookbook does
    # for a second attempt; main keeps the abandoned step 3.
    repo.create_branch("attempt-2", at=target)
    retry = repo.save_changes(branch="attempt-2", message="step 3 (retry): leave add alone",
                              changes={"NOTES.txt": "add() was right\n"}, parent=target,
                              author=velo.Author(f"agent-{RUN}"),
                              meta={"agent": {"run": RUN, "step": "3", "model": MODEL,
                                              "attempt": "2"}})
    assert repo.snapshot(retry.snapshot).parent == target
    assert repo.branch_tip("main") == agent.tip, "main still holds the bad edit"
    print("attempt-2 forks from step 2:", [e.message for e in repo.history(from_=retry.snapshot)])

    # Provenance survives: which run and command wrote the step we restored?
    print("provenance of step 2:", repo.snapshot_meta(target)["agent"])
    shutil.rmtree(tmp, ignore_errors=True)
    print("\nOK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
