#!/usr/bin/env python3
"""PreToolUse guard: edits to load-bearing engine files require human
sign-off (permissionDecision "ask"). See .claude/skills/vector3d-engine
for why each of these is guarded. Not a wall — a checkpoint."""

import json
import os
import re
import sys

GUARDED = [
    (r"^crates/vex-core/src/model\.rs$",
     ".vec binary format + loader/writer — breaking it orphans every shipped asset"),
    (r"^crates/vex-render/src/shaders/[^/]+\.wgsl$",
     "WGSL half of a Rust<->shader struct pair — must change in lockstep with Rust"),
    (r"^crates/vex-render/src/camera\.rs$",
     "CameraUniform layout (112 bytes) mirrored by camera.wgsl"),
    (r"^crates/vex-engine/src/collide\.rs$",
     "collision law (slide_capsule, came-from tie-break) — a shipped soft-lock lives here"),
]


def main() -> None:
    try:
        data = json.load(sys.stdin)
    except json.JSONDecodeError:
        return
    path = (data.get("tool_input") or {}).get("file_path") or ""
    if not path:
        return
    project = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
    rel = os.path.relpath(os.path.abspath(path), project).replace(os.sep, "/")
    for pattern, why in GUARDED:
        if re.match(pattern, rel):
            print(json.dumps({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "ask",
                    "permissionDecisionReason": (
                        f"LOAD-BEARING: {rel} — {why}. "
                        "Per .claude/skills/vector3d-engine this file needs "
                        "explicit human approval; bring a diagnosis, not a mutation."
                    ),
                }
            }))
            return


if __name__ == "__main__":
    main()
