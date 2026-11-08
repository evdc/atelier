#!/bin/sh
# Minimal stand-in for `claude -p --input-format stream-json --output-format stream-json`.
# A user message produces: init, a token delta, an assistant message and a
# permission request. The answer to that request ends the turn, echoing the
# control_response we received so tests can inspect it.
while IFS= read -r line; do
  case "$line" in
    *'"type":"user"'*)
      echo '{"type":"system","subtype":"init","session_id":"fake-native-1"}'
      echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}}'
      echo '{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"thinking","thinking":"","signature":"SIG"},{"type":"text","text":"hi"}]}}'
      echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}'
      ;;
    *'"control_response"'*)
      echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.01,\"duration_ms\":5,\"echo\":$line}"
      ;;
  esac
done
