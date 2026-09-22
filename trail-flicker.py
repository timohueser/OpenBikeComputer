# Count orange trail pixels per video frame; a frame whose count falls below half its neighbours' is a flicker.
import subprocess, sys
import numpy as np
for f in sys.argv[1:]:
    w, h = 201, 437
    raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", f, "-vf", f"scale={w}:{h}", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], capture_output=True).stdout
    fr = np.frombuffer(raw, np.uint8).reshape(-1, h, w, 3).astype(int)
    fr = fr[:, 60:360]  # skip control panels
    r, g, b = fr[..., 0], fr[..., 1], fr[..., 2]
    cnt = ((r > 200) & (g > 100) & (g < 190) & (b < 80)).sum(axis=(1, 2))
    nb = np.maximum(np.roll(cnt, 1), np.roll(cnt, -1))
    flick = [(i, cnt[i], nb[i]) for i in range(1, len(cnt) - 1) if nb[i] > 40 and cnt[i] < 0.5 * min(cnt[i - 1], cnt[i + 1])]
    print(f"{f}: frames={len(cnt)} median_orange_px={int(np.median(cnt))} flicker_frames={len(flick)} {flick[:5]}")
