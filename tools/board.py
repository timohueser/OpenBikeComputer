#!/usr/bin/env python3
"""Flash, attach, and diagnose the development board on macOS and Linux."""

import argparse
from contextlib import contextmanager
import fcntl
import glob
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shlex
import shutil
import subprocess
import sys


CHIP = "nRF54LM20A"
FLASH_FLAGS = ["--verify", "--disable-double-buffering"]


def lock_path():
    state = Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local/state"))
    return state / "obc" / "board.lock"


@contextmanager
def board_lock(path, command):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            handle.seek(0)
            raise RuntimeError(f"Board is in use: {handle.read().strip()}. Stop that session first.") from None
        handle.seek(0)
        handle.truncate()
        handle.write(f"PID {os.getpid()}, {Path.cwd()}, {shlex.join(command)}\n")
        handle.flush()
        # Closing our descriptor releases the lock only after any child also closes it.
        yield handle.fileno()


def probe_command(action, elf=None, probe=None, log=None, preverify=False):
    command = ["probe-rs", action, "--chip", CHIP, "--non-interactive"]
    if probe:
        command += ["--probe", probe]
    if action in ("run", "download"):
        command += FLASH_FLAGS
    if preverify:
        command.append("--preverify")
    if log:
        command += ["--target-output-file", str(log)]
    if elf:
        command.append(str(elf))
    return command


def inspect(command):
    if not shutil.which(command[0]):
        print(f"{command[0]} is not installed")
        return ""
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=15)
    except subprocess.TimeoutExpired:
        print(f"{shlex.join(command)} timed out")
        return ""
    return result.stdout + result.stderr


def usb_devices(tree):
    """Reduce macOS's USB inventory to the two board connections."""
    if isinstance(tree, dict):
        vendor, product = tree.get("idVendor"), tree.get("idProduct")
        if vendor == 0x1366 or (vendor, product) == (0x1209, 0x0001):
            yield {
                "connection": "J4 probe" if vendor == 0x1366 else "J3 device",
                "name": tree.get("USB Product Name", tree.get("IORegistryEntryName")),
                "serial": tree.get("USB Serial Number"),
                "vid_pid": f"{vendor:04x}:{product:04x}",
            }
        for value in tree.values():
            yield from usb_devices(value)
    elif isinstance(tree, list):
        for value in tree:
            yield from usb_devices(value)


def doctor():
    print("J4 — J-Link probe and VCOM (enumeration does not prove UART delivery):", flush=True)
    print(inspect(["probe-rs", "--version"]).strip())
    print(inspect(["probe-rs", "list"]).strip())
    ports = sorted(glob.glob("/dev/cu.usbmodem*") if sys.platform == "darwin" else glob.glob("/dev/ttyACM*"))
    print("Serial candidates: " + (", ".join(ports) or "none"))
    if ports:
        print(inspect(["lsof", "-nP", *ports]).strip())
    print("Probe processes (including sessions outside this harness):")
    print(inspect(["lsof", "-nP", "-a", "-c", "probe-rs", "-d", "cwd"]).strip())
    try:
        with board_lock(lock_path(), ["doctor"]):
            print("Harness lock: free")
    except RuntimeError as error:
        print(error)
    print("USB inventory (J3 is 1209:0001; enumeration is not a protocol health check):")
    if sys.platform == "darwin":
        output = inspect(["ioreg", "-a", "-p", "IOUSB", "-l", "-w", "0"])
        try:
            devices = list(usb_devices(plistlib.loads(output.encode())))
            print(json.dumps(devices, indent=2))
            if not any(device["connection"] == "J3 device" for device in devices):
                print("J3 device: not enumerated")
        except (plistlib.InvalidFileException, ValueError):
            print("USB inventory unavailable")
    elif shutil.which("lsusb"):
        print(inspect(["lsusb", "-d", "1209:0001"]).strip() or "No native device listed")
    else:
        print("USB inventory unavailable: install usbutils for lsusb")
    print("Recovery: close the owning host first. For J3, reconnect J3 and open a fresh session.")
    print("For VCOM, check the port, baud, debug-uart image and Board Configurator HWFC OFF.")
    print("If VCOM still drops writes, power-cycle the DK; an MCU reset does not reset J-Link.")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("run", "download", "attach", "reset", "doctor"))
    parser.add_argument("elf", nargs="?", type=Path)
    parser.add_argument("--probe", default=os.environ.get("PROBE_RS_PROBE"))
    parser.add_argument("--log", type=Path, help="save decoded RTT output (run/attach only)")
    parser.add_argument(
        "--preverify",
        action="store_true",
        help="read the image back first and program nothing when it already matches (run/download only)",
    )
    args = parser.parse_args(argv)
    if args.action in ("run", "download", "attach") and not args.elf:
        parser.error("this action needs the exact firmware ELF")
    if args.action in ("reset", "doctor") and args.elf:
        parser.error("this action does not take an ELF")
    if args.log and args.action not in ("run", "attach"):
        parser.error("--log is only available for run/attach")
    if args.preverify and args.action not in ("run", "download"):
        parser.error("--preverify is only available for run/download")
    if args.action == "doctor":
        return doctor()
    if args.elf:
        args.elf = args.elf.resolve(strict=True)
        print(f"ELF: {args.elf}\nSHA-256: {hashlib.sha256(args.elf.read_bytes()).hexdigest()}", flush=True)
    command = probe_command(args.action, args.elf, args.probe, args.log, args.preverify)
    print(shlex.join(command), flush=True)
    with board_lock(lock_path(), command) as lock_fd:
        # The child retains the lock if the wrapper is terminated during a flash or RTT session.
        result = subprocess.run(command, pass_fds=(lock_fd,))
    if result.returncode:
        print("Board command failed; run `obc board doctor` for connection diagnostics.", file=sys.stderr)
    return result.returncode


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, RuntimeError) as error:
        print(f"board: {error}", file=sys.stderr)
        sys.exit(1)
