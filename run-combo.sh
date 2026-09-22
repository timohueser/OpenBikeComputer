#!/bin/bash
# usage: run-combo.sh <cam> <trail> [seek] [seconds] [extra args...]
cd "$(dirname "$0")"
U=83FEF3C4-364E-4336-AEBE-1CE2CF6D587E
cam=$1; trail=$2; seek=${3:-5}; secs=${4:-14}; shift 4 2>/dev/null
name=evidence/runs/$cam-$trail
xcrun simctl launch --console-pty --terminate-running-process $U org.openbikecomputer.spike.flyover -mode $cam -trail $trail -seek $seek -autoplay YES -dist 2500 "$@" > $name.log 2>&1 &
sleep 3.5
xcrun simctl io $U recordVideo --codec h264 --force $name.mp4 >/dev/null 2>&1 &
rp=$!
sleep $secs
kill -INT $rp; wait $rp 2>/dev/null
# Frame timing: simctl records variable frame rate; long PTS gaps = no new frame = stall.
ffprobe -v error -select_streams v -show_entries frame=pts_time -of csv=p=0 $name.mp4 | awk -v n="$cam/$trail" '
  NR>1 && $1>1.0 { d=$1-p; if (d>0.05) g50++; if (d>0.1) g100++; if (d>m) m=d; c++ } { p=$1 }
  END { printf "%-18s frames=%d avgfps=%.1f gaps>50ms=%d gaps>100ms=%d maxgap=%.0fms\n", n, c, c/p, g50, g100, m*1000 }'
grep stats $name.log | sed 's/^/    /'
