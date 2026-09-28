#!/bin/bash
# Runs the bench plan in one installed browser, one `open` per step so the browser is in front for each.
# Needs `python3 web/serve.py` running. Run from speed/:
#   bash web/run_browser.sh "Google Chrome" [plan] [model]
#   bash web/run_browser.sh Safari "ort:1:cold,ort:1:cached" /work/onnx/ckpt-v1/model.int8.onnx
# A background or occluded window runs on efficiency cores: check the `focus` field and a
# tokenize time near 0.05 ms before you trust a row.
app=$1
plan=${2:-ort:1:cold,ort:4,ort:1:cached,rust:1:model.int8.onnx,rust:1:model.onnx}
model=${3:-/work/onnx/cut-50k/model.int8.onnx}
port=${PORT:-8765}
results=work/web_results.jsonl
v=$(date +%s)
n=$(echo "$plan" | tr ',' '\n' | wc -l | tr -d ' ')
for ((i = 0; i < n; i++)); do
  open -a "$app" "http://localhost:$port/web/index.html?runs=200&once=1&v=$v&step=$i&plan=$plan&model=$model"
  start=$(date +%s)
  until grep -q "\"step\": $i, \"v\": \"$v\"" $results 2>/dev/null; do
    [ $(($(date +%s) - start)) -gt 600 ] && { echo "timeout at step $i"; break; }
    sleep 3
  done
done
grep "\"v\": \"$v\"" $results | python3 -c '
import json, sys
for line in sys.stdin:
    r = json.loads(line)
    print(r["step"], r["runtime"], r["label"], "threads", r["threadsEffective"], "load %.0f ms%s, first %.1f, median %.1f, p95 %.1f, tokenize %.2f, mismatches %d, memory %s MB, %s | %s" % (
        r["fetchMs"] + r["createMs"], " (cache)" if r["fromCache"] else "", r["firstMs"], r["medianMs"], r["p95Ms"],
        r["tokenizeMedianMs"], r["tokMismatch"], r["memoryMb"] and round(r["memoryMb"]), r["focus"], r["ua"][-40:]))
'
