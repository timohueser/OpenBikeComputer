#!/usr/bin/env python3
"""On-glass before/after bench for the line-stroke rewrite.

Drives the STM32F429 over its USB-CDC debug link (the `obc-platform::debug_usb` protocol):
navigates Home → load route0 → ride, then sweeps the map's exact meters-per-pixel with `Z <mpp>`
(each `Z` forces one redraw) and reads back the per-frame render telemetry `T ...`. Reports the
overlay stage (`overlay_us` = route + breadcrumb + marker stroke — what this PR changes) and the
base-map `draw_us` per zoom. Run it against the baseline firmware and the PR firmware and diff.

    python3 device_overlay_bench.py [/dev/cu.usbmodemobc_f429X]
"""
import glob
import statistics
import sys
import time

import serial

MPPS = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0]
BAUD = 115200

# `T frame_us lod feat_drawn feat_tried feat_dropped chunks hits misses reads bytes
#    collect_us read_us sort_us draw_us overlay_us mpp_milli`  (indices 0..=16)
DRAW_US, OVERLAY_US, MPP_MILLI = 14, 15, 16


def find_port():
    if len(sys.argv) > 1:
        return sys.argv[1]
    cands = sorted(glob.glob("/dev/cu.usbmodemobc_f429*"))
    if not cands:
        sys.exit("no /dev/cu.usbmodemobc_f429* port — is the board plugged in and flashed with --features debug-usb?")
    return cands[0]


def parse_t(line):
    p = line.split()
    if len(p) < 17 or p[0] != "T":
        return None
    try:
        return (int(p[DRAW_US]), int(p[OVERLAY_US]), int(p[MPP_MILLI]))
    except ValueError:
        return None


def main():
    port = find_port()
    ser = serial.Serial(port, BAUD, timeout=0.2)
    print(f"# port {port}")

    def send(s):
        ser.write((s + "\n").encode())
        ser.flush()

    def tap():  # encoder Press = down then a quick up
        send("K e d")
        time.sleep(0.08)
        send("K e u")
        time.sleep(0.45)

    def drain(seconds):
        end = time.time() + seconds
        rows = []
        while time.time() < end:
            ln = ser.readline().decode(errors="replace").strip()
            t = parse_t(ln)
            if t:
                rows.append(t)
        return rows

    time.sleep(0.5)
    ser.reset_input_buffer()
    tap()  # Home Press → Route menu
    tap()  # Route menu Press → load route0 → ride (Map)
    time.sleep(1.5)  # let the SD geometry + ride log open

    print(f"{'mpp':>6} {'overlay_us':>11} {'draw_us':>9} {'n':>4}")
    for mpp in MPPS:
        ser.reset_input_buffer()
        send(f"Z {mpp}")
        time.sleep(0.3)
        rows = drain(2.0)
        want = round(mpp * 1000)
        use = [r for r in rows if abs(r[2] - want) <= 1] or rows
        if use:
            ov = round(statistics.median(r[1] for r in use))
            dr = round(statistics.median(r[0] for r in use))
            print(f"{mpp:>6.0f} {ov:>11} {dr:>9} {len(use):>4}")
        else:
            print(f"{mpp:>6.0f} {'(no telemetry)':>11}")
    ser.close()


if __name__ == "__main__":
    main()
