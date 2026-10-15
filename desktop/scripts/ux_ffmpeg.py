#!/usr/bin/env python3
"""Inject a frame-extraction fault only for the isolated synthetic UX library.

All successful operations still run the installed ffmpeg binary. This wrapper is
never installed in the user's PATH; the validation app opts into its directory.
"""
import json
import os
from pathlib import Path
import shutil
import sys
import time



def real_tool(name):
    """The installed tool on PATH, skipping the fixture directory that holds this wrapper."""
    here = Path(__file__).resolve().parent
    path = os.pathsep.join(d for d in os.environ.get("PATH", "").split(os.pathsep)
                           if d and Path(d).resolve() != here)
    return shutil.which(name, path=path)


args = sys.argv[1:]
root = Path(os.environ.get("COURSE2MD_UX_FIXTURE_ROOT", "/tmp/course2md-ux-validation"))
binary = real_tool("ffmpeg")
if binary is None:
    sys.exit("Installed ffmpeg is unavailable")

is_fixture = any(str(root) in arg or str(root.resolve()) in arg for arg in args)
if is_fixture and "-frames:v" in args:
    try:
        policy = json.loads((root / "tool-mode.json").read_text())
    except (OSError, ValueError):
        policy = {}
    mode = policy.get("ffmpeg", "valid")
    with (root / "tool-requests.jsonl").open("a") as stream:
        stream.write(json.dumps({"time": time.time(), "operation": "extract-frame", "mode": mode}) + "\n")
    if mode == "screenshot-failure":
        time.sleep(min(float(policy.get("seconds", 0)), 120))
        sys.exit("Synthetic acceptance fault: could not write the extracted frame (Permission denied)")
    if mode == "slow-screenshot":
        time.sleep(min(float(policy.get("seconds", 20)), 120))

os.execv(binary, [binary, *args])
