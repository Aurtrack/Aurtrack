#!/usr/bin/env bash
# 竖切冒烟测试：CLI 渲染一条正弦轨，MCP 暴露 7 个 tool。
# 用法：bash scripts/smoke.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT="target/smoke"
mkdir -p "$OUT"
rm -f "$OUT/cli.jsonl" "$OUT/cli.wav" "$OUT/mcp.jsonl"

cargo build --workspace --quiet

printf '%s\n' \
  '{"cmd":"project.create","args":{"name":"smoke"}}' \
  '{"cmd":"track.create","args":{"name":"bass","source":"sine","frequency":110}}' \
  '{"cmd":"device.add","args":{"track_id":"track-1","kind":"gain"}}' \
  '{"cmd":"param.set","args":{"path":"/tracks/track-1/devices/device-1/params/gain","value":0.5}}' \
  "{\"cmd\":\"render.segment\",\"args\":{\"duration_seconds\":0.1,\"path\":\"$OUT/cli.wav\"}}" \
  | target/debug/aurtrack --stdin >"$OUT/cli.jsonl"

if [ "$(grep -c '"ok":true' "$OUT/cli.jsonl")" -ne 5 ]; then
  echo "CLI 没有全部返回 ok" >&2
  cat "$OUT/cli.jsonl" >&2
  exit 1
fi

if [ ! -s "$OUT/cli.wav" ]; then
  echo "没有生成 WAV" >&2
  exit 1
fi

printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | target/debug/aurtrack-mcp >"$OUT/mcp.jsonl"

node -e '
const fs = require("fs");
const lines = fs.readFileSync(process.argv[1], "utf8").trim().split("\n").map(JSON.parse);
const list = lines.find((line) => line.id === 2);
if (!list || list.result.tools.length !== 7) {
  console.error("MCP tools/list 没有返回 7 个 tool");
  process.exit(1);
}
console.log("MCP tools:", list.result.tools.map((tool) => tool.name).join(", "));
' "$OUT/mcp.jsonl"

echo "OK: WAV $(wc -c <"$OUT/cli.wav") 字节；CLI 5 条命令全部成功"
