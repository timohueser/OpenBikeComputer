#!/bin/sh
# Screenshot a local page with headless Chrome: sh kit/shot.sh page.html out.png [width] [height] [light|dark]
W=${3:-1400}; H=${4:-2400}; T=${5:-light}
S=1; [ "$T" = dark ] && S=0
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu --hide-scrollbars \
  --blink-settings=preferredColorScheme=$S --window-size="$W,$H" --virtual-time-budget=4000 \
  --screenshot="$2" "file://$(cd "$(dirname "$1")" && pwd)/$(basename "$1")" 2>/dev/null
echo "shot $2"
