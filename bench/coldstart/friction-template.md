# Cold-start friction log: <wedge>, <date>

Agent and model: <name, version>
Velo version and install source: <wheel/tarball/tag, SHA256 verified yes/no>
Time to first successful `save`: <mm:ss>
Total time: <mm:ss> of the 60 minute bound
Independent check (`check.py` verdict): <paste the JSON, or its `ok` field>

One row per event. A "defect" names the doc paragraph or API at fault.

| Time | Guess or error | Doc or API defect named | Severity | Consciously deferred |
|------|----------------|-------------------------|----------|----------------------|
| 00:00 | guess / error / missing method: what happened | e.g. README "Quick start" omits X; `Repo.foo` does not exist | blocker / major / minor | no, or why and until when |

Severity: blocker stopped the task or needed a hint; major cost over five
minutes or a wrong result that was noticed; minor was a guess that turned out
right or a confusing message.

Summary: <three lines: what to fix first>
