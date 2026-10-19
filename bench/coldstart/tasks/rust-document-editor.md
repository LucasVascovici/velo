# Task: Rust document editor

Documentation given: `README.md`, `docs/cookbook/document-editor.md`.
Installed from: the crate the agent adds to its own `Cargo.toml`, as the
README says; the agent is not given a checkout.

## Task (give the agent only this sentence)

A collaborative editor on velo: drafts are branches, saving is a snapshot, and
blame tells you who wrote each sentence.

Use the actor name `coldstart-rust` for the author of the saved work. (Give
this name to the agent together with the sentence.)

## Acceptance (for the checker; not shown to the agent)

- The repository has a draft branch and at least one snapshot saved from Rust.
- `velo fsck` is clean.
- History is non-empty.
- Blame of the document the agent wrote names `coldstart-rust`.

```bash
python ../check.py <repo-dir> --actor coldstart-rust --file <document path>
```

Source of the sentence: the introduction of `docs/cookbook/document-editor.md`.
