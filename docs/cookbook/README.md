# velo cookbooks

Each cookbook is a short narrative plus a script that runs against the Python
binding. CI executes every script after the binding's tests, and the scripts
assert what the prose claims, so these pages cannot drift from the behaviour.

| Cookbook | Script | What it shows |
| --- | --- | --- |
| [Agent checkpointing](agent-checkpointing.md) | `bindings/python/examples/agent_checkpointing.py` | A snapshot per tool call, a branch per attempt, pick a winner by metadata, blame by run. Includes the MCP tool names. |
| [Config registry](config-registry.md) | `bindings/python/examples/config_registry.py` | Versioned configs, rollback by carry-forward, an audit trail, and tamper evidence. |
| [Document editor](document-editor.md) | `bindings/python/examples/document_editor.py` | Drafts as branches, a rename edge, a conflict resolved with bytes, blame per line. |

To run one locally, build the binding as described in
[`bindings/python/README.md`](../../bindings/python/README.md) and then run
`python bindings/python/examples/<name>.py` with the venv's Python.
