#!/usr/bin/env python3
"""AT-007 spike: does `updatedPermissions` in a can_use_tool allow stop the next prompt?

Drives `claude` headless over stream-json, the way crates/atelier-driver does:
1. asks for a Bash command; when the permission request arrives, saves its
   `permission_suggestions` and answers allow with `updatedPermissions` set to
   the first suggestion (the "Always allow" click);
2. asks for the same command again in the same session and reports whether
   CC prompts a second time.

Usage: python3 spike.py [claude-binary] > out.jsonl
Every line the CLI sends and every reply we write is echoed to stdout as
{"dir": "<-" | "->", "msg": ...}; the final line is {"dir": "summary", ...}.
Needs an authenticated `claude` on PATH. Run it in a throwaway directory.
"""

import json
import subprocess
import sys

BIN = sys.argv[1] if len(sys.argv) > 1 else "claude"
# `echo` is read-only and auto-approved in default mode, so it never prompts. A write does.
COMMAND = "touch atelier-spike-marker"
PROMPT = f"Run `{COMMAND}` using the Bash tool, then reply with the word done."

proc = subprocess.Popen(
    [BIN, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
     "--permission-prompt-tool", "stdio", "--permission-mode", "default"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1,
)


def log(direction, msg):
    print(json.dumps({"dir": direction, "msg": msg}), flush=True)


def send(msg):
    log("->", msg)
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()


def user_turn(text):
    send({"type": "user", "message": {"role": "user", "content": text}})


prompts_seen = []  # can_use_tool requests, in order
turns = 0
prompts_before_second_turn = 0
user_turn(PROMPT)
for line in proc.stdout:
    msg = json.loads(line)
    log("<-", msg)
    if msg.get("type") == "control_request" and msg["request"].get("subtype") == "can_use_tool":
        req = msg["request"]
        prompts_seen.append(req)
        suggestions = req.get("permission_suggestions") or []
        allow = {"behavior": "allow", "updatedInput": req.get("input", {})}
        if len(prompts_seen) == 1 and suggestions:
            allow["updatedPermissions"] = [suggestions[0]]  # the "Always allow" click
        send({"type": "control_response",
              "response": {"subtype": "success", "request_id": msg["request_id"], "response": allow}})
    elif msg.get("type") == "result":
        turns += 1
        if turns == 1:
            prompts_before_second_turn = len(prompts_seen)
            user_turn(f"Run `{COMMAND}` again using the Bash tool, then reply with the word done.")
        else:
            break

proc.stdin.close()
proc.wait(timeout=30)
second_turn_prompts = len(prompts_seen) - prompts_before_second_turn
log("summary", {
    "first_turn_prompts": prompts_before_second_turn,
    "second_turn_prompts": second_turn_prompts,
    "suggestions_offered": prompts_seen[0].get("permission_suggestions") if prompts_seen else None,
    "always_allow_stuck": bool(prompts_seen) and second_turn_prompts == 0,
})
