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
mcp_config=
prev=
for arg in "$@"; do
  [ "$arg" = "--resume" ] && resumed=true
  [ "$prev" = "--mcp-config" ] && mcp_config=$arg
  prev=$arg
done
while IFS= read -r line; do
  case "$line" in
    *'"type":"user"'*)
      echo "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"fake-native-1\",\"resumed\":$resumed}"
      # `MCP:<tool>:<json args>` calls the Atelier MCP server the way the real CLI would (URL and token come
      # from --mcp-config) and prints the reply as a tool_result. The turn then ends with no permission request.
      case "$line" in
        *MCP:*)
          # The string ends at the first quote that isn't escaped (the driver may order keys any way).
          spec=$(printf '%s' "${line#*MCP:}" | sed 's/\([^\\]\)".*/\1/')
          # An envelope puts more lines after the call; a JSON `\n` ends it.
          spec=$(printf '%s' "$spec" | sed 's/\\n.*//')
          tool=${spec%%:*}
          args=$(printf '%s' "${spec#*:}" | sed 's/\\"/"/g')
          url=$(printf '%s' "$mcp_config" | sed -n 's/.*"url":"\([^"]*\)".*/\1/p')
          token=$(printf '%s' "$mcp_config" | sed -n 's/.*Bearer \([^"]*\)".*/\1/p')
          reply=$(curl -s -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
            -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$tool\",\"arguments\":$args}}" "$url")
          # printf, not echo: the reply has \n escapes that some shells' echo would expand.
          printf '%s\n' "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"mcp1\",\"content\":$reply}]}}"
          echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0,"duration_ms":1}'
          continue ;;
      esac
      echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}}'
      echo '{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"thinking","thinking":"","signature":"SIG"},{"type":"text","text":"hi"}]}}'
      echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}'
      ;;
    *'"control_response"'*)
      echo "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.01,\"duration_ms\":5,\"echo\":$line}"
      ;;
  esac
done
