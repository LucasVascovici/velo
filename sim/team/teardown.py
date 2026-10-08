#!/usr/bin/env python
"""Stop the serve-http process started by setup.py:  python sim/team/teardown.py <SIMDIR>"""
import os, signal, subprocess, sys
from pathlib import Path

pid_file = Path(sys.argv[1]) / "server.pid"
if pid_file.exists():
    pid = int(pid_file.read_text())
    if os.name == "nt":
        subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)
    else:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    print("stopped", pid)
