# An outside agent loop with velo wired in

One third-party agent loop, with velo added using only the public docs. It
checks the 20.11 question "what does the API lack?" against a loop this project
did not write.

## The loop

| | |
|---|---|
| Project | [mini-swe-agent](https://github.com/SWE-agent/mini-swe-agent) (SWE-agent team) |
| URL | https://github.com/SWE-agent/mini-swe-agent |
| Commit | `04d809ceab9df28f9adaed044884180159172930` (package version 2.4.6) |
| License | MIT, kept in [`LICENSE-mini-swe-agent.md`](LICENSE-mini-swe-agent.md) |
| Used as | `minisweagent.agents.default.DefaultAgent` (a 190-line loop), unmodified |

It is not vendored: [`requirements.txt`](requirements.txt) installs it from the
pinned commit. Copyright (c) 2025 Kilian A. Lieret and Carlos E. Jimenez. Only
`run_example.py` here is velo's.

## What `run_example.py` does

`VeloAgent` subclasses `DefaultAgent` and overrides `execute_actions`. After
each tool call it diffs the workspace against what the last snapshot holds and
calls `save_changes` with only the changed files (`None` for deleted ones),
`parent` set to the previous checkpoint, an author `agent-run-1`, and `agent`
metadata: run, step, model and the command. A step-0 snapshot of the empty
workspace comes first. The save is in a `finally` because the loop's submit
command raises out of `execute_actions`.

Then the script restores: it empties the workspace, calls `materialize` on the
step-2 snapshot (before the bad edit at step 3), checks the bytes and that the
restored test passes, and carries on from there on a new branch `attempt-2`
while `main` keeps the abandoned step. Metadata of the restored step is printed.

**No LLM and no network at run time.** The loop's own `DeterministicModel`
(`minisweagent.models.test_models`) replays five scripted assistant messages:
create a module, add a test, make a bad edit, run the test, submit.

## Run it

```bash
python -m venv .venv                       # .venv/Scripts on Windows
.venv/bin/pip install maturin
.venv/bin/maturin develop --release --manifest-path ../../bindings/python/Cargo.toml
.venv/bin/pip install -r requirements.txt  # needs network once, for the loop
.venv/bin/python run_example.py
```

Expected: `agent exit: Submitted`, four checkpoints (steps 0 to 3), the workspace
before and after the restore, the `attempt-2` history, the step-2 provenance,
and `OK`. Run on Windows 11, Python 3.12.8, velo built from this checkout. The
loop's own tests were not run: it is installed unmodified, from its commit.

## What the API lacked

Written with README.md, `docs/cookbook/agent-checkpointing.md` and the pdoc
reference generated as `apidocs.yml` does (no release exists yet, so there is no
`api-python.zip` to download; see item 1). The velo source was not read. Each
item is a defect in the docs or the API.

1. **The API reference cannot be obtained.** README ("Further reading") sends
   you to `api-python.zip` on a release page. There is no release, so I
   reproduced the CI recipe from `.github/workflows/apidocs.yml` by hand. Expected
   a link that works today, or the reference built into the Python README.
2. **No way to save "what the tool did to a directory".** The cookbook's
   `save_changes` takes `changes` "that the tool call touched", which assumes the
   tool knows. A bash-style loop does not: it runs a command in a shell. I
   expected a helper that diffs a directory against a snapshot (or `save_dir`).
   I did instead keep a mirror of file bytes, rescanned after each call and
   diffed it myself. Every agent loop that shells out will rewrite this.
3. **Line endings are rewritten and undocumented.** A file saved as `b"x\r\ny\r\n"`
   reads back and materializes as `b"x\ny\n"`; a file with a NUL byte keeps its
   CRLF. Neither the README, the cookbook nor the reference says velo normalizes
   text. For a "restore to the exact earlier state" feature that matters; I
   expected a statement and an opt-out, and instead found it by asserting on bytes.
4. **`materialize` has no docstring and unstated semantics.** The reference
   gives only a signature. I expected to learn whether it overwrites existing
   files, deletes files absent from the snapshot, and what the `int` is. By
   experiment it overwrites, leaves strays alone, and returns a file count, so a
   true "restore" needs the caller to empty the directory first (I did). Expected
   a `restore(id, dest)` that makes the directory equal the snapshot, since that
   is the whole point of checkpointing.
5. **`Repo.init` says nothing about what it makes.** No docstring, and the README
   example (`Repo.init("work")`) reads as a working directory while the cookbook
   says "without a working tree". It creates `.velo` and `.veloignore` inside the
   given path. I could not tell from the docs whether the workspace itself should
   be the repo, so I kept the store in a separate directory. Expected a sentence
   on when to use a separate store.
6. **`history(branch=...)` is not ancestry, and the doc does not say what it is.**
   The reference says only "Snapshots, newest first". After restoring step 2 and
   saving on `main` with `parent=<step 2>`, `history(branch="main")` still listed
   the abandoned step; `history(from_=<tip>)` gave the true chain. The cookbook
   uses `from_=`, but nothing explains the difference, and a restore-then-continue
   flow hits it. Expected the semantic stated.
7. **Saving with a `parent` that is not the branch tip is not discussed.** The
   branch tip moved to the new snapshot and the abandoned one stayed on the
   branch label. The cookbook only forks with `create_branch(at=...)`. I expected
   a rule for "retry from an earlier step on the same branch" (error, or fork) and
   used a new branch to stay on documented ground.
8. **Empty `changes` and `parent=None` are accepted silently.** The first
   checkpoint is `save_changes(changes={})`, and a second `parent=None` save onto a
   branch that already has a tip also succeeded without error. The docs do not say
   either is allowed. Expected an error or a documented rule for creating the root
   and for a stale `parent`.
9. **Metadata limits are undocumented.** `meta` is `Dict[str, Dict[str, str]]`, so
   step numbers must be stringified (the signature said so, the cookbook shows
   only strings), but there is no stated limit on value size or key charset. A
   100 KB value was accepted. I truncated the command to 200 characters out of
   caution; expected a documented bound.
10. **Exceptions are not documented.** Every error class shows the inherited
    text "Common base class for all non-exit exceptions" in the reference, and no
    method lists what it raises. A loop that wants to retry or degrade cannot tell
    `Locked` from `DatabaseError` from `Conflicts` without trying.

Not lacking, with evidence: the checkpoint itself. `save_changes` with a dict of
changed files, an `Author` and `meta` worked first time from the cookbook snippet,
`snapshot_meta` returned the metadata, and `branch_tip`, `create_branch`,
`snapshot` and `history(from_=...)` behaved as documented.
