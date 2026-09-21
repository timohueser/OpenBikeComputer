"""Photo, Peak View before, Peak View after — all three over the same angular window.

No camera fitting: the heading, the lens and the pitch are stated per photo, the photo is cropped
to the chart's 240:222 aspect about the optical axis, and both charts are rendered over exactly
that window. What is left to compare is the shape of the terrain.
"""
import math
import os
import pathlib
import subprocess
import sys

from PIL import Image, ImageDraw, ImageOps

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Baked containers, reference DEMs and output images. They are large and are not in Git;
# ../README.md says how to make them.
WORK = os.environ.get("PEAKVIEW_WORK", "/tmp/peak-view")

SP = WORK
BIN = ROOT / "pano/target/release"
U = str(ROOT / "photos")
CHART_W, CHART_H = 240, 222
# 2 m swissALTI3D at each viewpoint, the best figure available for the observer.
GROUND = {"below-titlis": 2564.4, "below-west": 2564.4, "top-south": 2591.4, "top-north": 2591.4}
TONES = {0: 255, 1: 170, 2: 85, 3: 0}


def read_pgm(path):
    d = open(path, "rb").read()
    parts, i = [], 0
    while len(parts) < 4:
        while d[i:i + 1].isspace():
            i += 1
        if d[i:i + 1] == b"#":
            while d[i:i + 1] != b"\n":
                i += 1
            continue
        j = i
        while not d[j:j + 1].isspace():
            j += 1
        parts.append(d[i:j])
        i = j
    return int(parts[1]), int(parts[2]), d[i + 1:]


def chart(obcd, lat, lon, heading, fov_deg, centre_deg, span_deg, out):
    subprocess.run([f"{BIN}/pano", obcd, str(round(lat * 1e6)), str(round(lon * 1e6)),
                    out, f"{heading:.3f}", str(round(fov_deg * 4)), str(round(centre_deg * 4)),
                    str(round(span_deg * 4))], capture_output=True, check=True)
    w, h, g = read_pgm(out)
    return Image.frombytes("L", (w, h), g).convert("RGB")


def panel(name, photo, pos, heading, hfov_full, pitch, scale=4):
    """`hfov_full` is the whole frame's field; `pitch` is the optical axis, degrees above level."""
    im = ImageOps.exif_transpose(Image.open(photo)).convert("RGB")
    W, H = im.size
    f = (W / 2) / math.tan(math.radians(hfov_full / 2))
    crop_w = H * CHART_W / CHART_H
    if crop_w > W:
        crop_w, H = W, W * CHART_H / CHART_W
    box = ((W - crop_w) / 2, (im.height - H) / 2, (W + crop_w) / 2, (im.height + H) / 2)
    shot = im.crop([round(v) for v in box]).resize((CHART_W * scale, CHART_H * scale), Image.LANCZOS)
    hfov = 2 * math.degrees(math.atan(crop_w / 2 / f))
    vfov = 2 * math.degrees(math.atan(H / 2 / f))
    print(f"{name}: heading {heading}  window {hfov:.1f} x {vfov:.1f} deg  centre {pitch:+.1f} deg")

    out = [shot]
    for obcd, label in ((f"{WORK}/eng_plain.obcd", "before"), (f"{WORK}/eng_crest.obcd", "after")):
        c = chart(obcd, *pos, heading, hfov, pitch, vfov, f"{WORK}/_sbs.pgm")
        out.append(c.resize((CHART_W * scale, CHART_H * scale), Image.NEAREST))
    for im2, label in zip(out, ("photo", "before — Copernicus GLO-30", "after — swissALTI3D crest planes")):
        ImageDraw.Draw(im2).text((10, 8), label, fill=(255, 0, 0))
    g = Image.new("RGB", (out[0].width, out[0].height * 3 + 16), (255, 255, 255))
    for i, im2 in enumerate(out):
        g.paste(im2, (0, i * (out[0].height + 8)))
    g.save(f"{WORK}/sbs_{name}.png")
    return g


if __name__ == "__main__":
    BELOW, TOP = (46.85212, 8.41904), (46.85227, 8.41959)
    CASES = {
        # Heading and pitch from `refine`: the landmark bearings set the hint, the photo's own
        # skyline against the 2 m LiDAR settles the two free numbers. The lens is the iPhone main.
        "below-titlis": (f"{U}/rigidalstock-below-titlis.jpg", BELOW, 191.25, 69.4, -2.84),
        "below-west":   (f"{U}/rigidalstock-below-west.jpg", BELOW, 222.25, 69.4, -0.26),
        "top-south":    (f"{U}/rigidalstock-top-south.jpg", TOP,   146.25, 69.4, -2.18),
        "top-north":    (f"{U}/rigidalstock-top-north.jpg", TOP,   305.75, 69.4, -0.36),
    }
    for n in (sys.argv[1:] or CASES):
        panel(n, *CASES[n])


def refine(name, photo, pos, heading, hfov_full, floor, span=10.0):
    """Heading and pitch from the photo's own skyline, with the lens and the window fixed.

    Only two numbers are free, so this cannot wander onto the wrong mountains the way a full
    camera fit can; the landmark bearings already pinned the heading to within a few degrees.
    """
    import numpy as np
    sys.path.insert(0, SP)
    from fit import load, photo_skyline
    im = ImageOps.exif_transpose(Image.open(photo)).convert("RGB")
    W, H = im.size
    f = (W / 2) / math.tan(math.radians(hfov_full / 2))
    crop_w = H * CHART_W / CHART_H
    box = ((W - crop_w) / 2, 0, (W + crop_w) / 2, H)
    shot = im.crop([round(v) for v in box]).resize((CHART_W * 4, CHART_H * 4), Image.LANCZOS)
    hfov = 2 * math.degrees(math.atan(crop_w / 2 / f))
    vfov = 2 * math.degrees(math.atan(H / 2 / f))
    sk = photo_skyline(shot, floor=floor)
    cols = np.nonzero(sk >= 0)[0]
    # The photo's skyline as an elevation angle relative to the optical axis.
    photo_ang = -(sk[cols] - shot.height / 2) / shot.height * vfov
    out = subprocess.run([f"{BIN}/hybridsky", f"{WORK}/lidar", f"{WORK}/dem",
                          f"{pos[0]:.6f}", f"{pos[1]:.6f}", f"{GROUND[name] + 1.6:.2f}",
                          f"{heading - hfov:.3f}", f"{heading + hfov:.3f}", "0.05"],
                         capture_output=True, text=True, check=True).stdout
    a = np.array([[float(v) for v in line.split()] for line in out.splitlines()])
    best = None
    for dyaw in np.arange(-span, span + 0.01, 0.25):
        want = heading + dyaw + (cols / shot.width - 0.5) * hfov
        truth = np.interp(want, a[:, 0], a[:, 1])
        d = truth - photo_ang
        pitch = float(np.median(d))
        err = np.sort(np.abs(d - pitch))[: int(0.8 * len(d))]
        score = float(np.sqrt(np.mean(err ** 2)))
        if best is None or score < best[0]:
            best = (score, heading + dyaw, pitch)
    print(f"  {name}: heading {best[1]:.2f} (hint {heading}), pitch {best[2]:+.2f}, spread {best[0]:.2f} deg")
    return best[1], best[2]
