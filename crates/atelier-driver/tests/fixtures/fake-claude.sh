#!/bin/sh
# Minimal stand-in for `claude -p --input-format stream-json --output-format stream-json`.
# A user message produces: init, a token delta, an assistant message and a
# permission request. The answer to that request ends the turn, echoing the
# control_response we received so tests can inspect it. The init message says
# whether this process was started with --resume.
# The one-shot titler (`claude -p --no-session-persistence ... --output-format json`) gets a fixed title.
case "$*" in
  *--no-session-persistence*)
    echo '{"type":"result","subtype":"success","is_error":false,"result":"Login rate limits"}'
    exit 0 ;;
esac
# Tests read what the agent was started with from its git dir (untracked, so it stays out of Changes).
echo "$*" > "$(git rev-parse --git-dir 2>/dev/null)/atelier-fake-args" 2>/dev/null
resumed=false
for arg in "$@"; do
  [ "$arg" = "--resume" ] && resumed=true
done
while IFS= read -r line; do
  case "$line" in
    *'"type":"user"'*)
      echo "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"fake-native-1\",\"resumed\":$resumed}"
      echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}}'
      echo '{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"thinking","thinking":"","signature":"SIG"},{"type":"text","text":"hi"}]}}'
      echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}'
      ;;
    *'"control_response"'*)
      echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.01,\"duration_ms\":5,\"echo\":$line}"
      ;;
  esac
done
