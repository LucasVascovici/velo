# Task: Node config registry

Documentation given: `README.md`, `docs/cookbook/config-registry.md`.
Installed from: the Node tarball from the release.

## Task (give the agent only this sentence)

A registry publishes versions of `configs/service.yaml`, rolls back cheaply,
and can answer "who approved this?" in a way nobody can rewrite.

Use the actor name `coldstart-node` for the author of every published version.
(Give this name to the agent together with the sentence.)

## Acceptance (for the checker; not shown to the agent)

- The repository holds at least two published versions of
  `configs/service.yaml`, saved through the installed Node package.
- `velo fsck` is clean.
- History is non-empty.
- Blame of `configs/service.yaml` names `coldstart-node`.

```bash
python ../check.py <repo-dir> --actor coldstart-node --file configs/service.yaml
```

Source of the sentence: the introduction of `docs/cookbook/config-registry.md`.
