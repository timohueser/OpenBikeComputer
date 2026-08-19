#!/usr/bin/env python3
"""Send one file into a board's flat store over the DK's VCOM UART.

The device half is a mode of `firmware/obc-fw-nrf54l/src/bin/flat_store_bench.rs`, which is where
the wire is documented normatively; this is its host counterpart. It exists for one reason: a board
session needs a real packed map on a real flat store, and on this rig no transport can put one there
— USB is still protocol v2, BLE v4's phone client is not ready, and the host has no card reader.

    python3 tools/bench_ingest.py --port /dev/cu.usbmodem0010513330133 \\
        --file "$(python3 tools/fixtures.py resolve monaco-upahead | awk '/^map/ {print $2}')" \\
        --kind map --name monaco.obcm

Start this **before** flashing: it blocks on the device's READY advertisement, so the natural order
is to leave it waiting and then `cargo run --release --bin flat_store_bench` in another shell.

One failure mode is worth knowing before it happens, because it is silent on both sides and it ends
with a wiped card. This host transmits **only after decoding a valid READY**, so if `--baud` does not
match the device's `INGEST_BAUD` it decodes nothing, sends nothing, and simply waits — while the
device sees an idle line, concludes nobody is there, and starts its destructive measurement run. The
signature is exactly that pair: RTT saying `nobody answered` while this script says it is waiting.
Check `--baud` first, not the cable.

Everything above `Link` is pure framing with no pyserial in it, which is what
`tools/tests/test_bench_ingest.py` drives against a fake device.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import os
from pathlib import Path
import sys
import time
import zlib

# ── the wire (mirror of flat_store_bench.rs's module docs) ──────────────────────────────────────

MAGIC = b"OBCI"
VERSION = 1

TAG_READY = ord("R")
TAG_GONE = ord("G")
TAG_HEADER = ord("H")
TAG_DONE = ord("D")
TAG_FAIL = ord("E")

STATUS_ACK = 0x06
STATUS_NAK = 0x15

READY_BYTES = 14
HEADER_BYTES = 72
RESULT_BYTES = 42

NAME_CAPACITY = 48

# What a retry waits for the device, against its ten-second inter-object window. The first attempt
# uses `--wait` instead: that one is waiting for a person to flash the board.
RETRY_WAIT = 15.0

# `FLAT_Store_Format.md` §3.1. The device decodes the same table and refuses anything else, so this
# is a convenience for the command line rather than a second authority.
KINDS = {
    "route": 1,
    "trip": 2,
    "ride": 3,
    "weather": 4,
    "map": 5,
    "map-set": 6,
    "update": 7,
    "rollback": 8,
}

# The device's refusal reasons, by their wire number.
REASON_PAYLOAD_CRC = 9
REASON_COMMIT = 10
REASON_LINK = 11

REASONS = {
    0: "no reason given",
    1: "the device does not speak this wire version",
    2: "the header's CRC did not check — the framing is out of step",
    3: "the object kind is not one of FLAT_Store_Format.md §3.1's",
    4: "the name is over 48 bytes or is not UTF-8",
    5: "a zero-length payload",
    6: "the card did not come up writable and initialization cannot repair it",
    7: "the payload does not fit the card's free extents",
    8: "the store refused a chunk",
    REASON_PAYLOAD_CRC: "the payload CRC did not match — nothing was committed",
    REASON_COMMIT: "the publishing commit was refused",
    REASON_LINK: "the link timed out or the UARTE reported an error",
}

# Failures the device recovered from cleanly — it cancelled the reservation and went back to
# advertising — so sending the whole object again is the right response and costs nothing but time.
#
# Two reasons are deliberately NOT here, for opposite causes:
#   - a refused commit (10) is not self-cleaning: the device ends the session holding the extents
#     until a remount, so a retry fails differently and for the wrong reason;
#   - a refused chunk (8) IS self-cleaning — the device cancels and carries on — but retrying it is
#     pointless. It means the store would not take those bytes at that offset, and the retry sends
#     the identical bytes to the identical offset. Only a different card or a different payload
#     changes the answer, so the operator should see the refusal rather than watch it repeat.
RETRIABLE = frozenset({REASON_PAYLOAD_CRC, REASON_LINK})

# Why the device stopped listening. `GONE` is READY's shape with another tag, sent on every exit
# path, so a host that would otherwise time out learns which of these it is.
GONE_REASONS = {
    1: "the ten-second window closed and THE DESTRUCTIVE MEASUREMENT RUN IS STARTING — reset the "
    "board now, then re-run this",
    2: "the session is over (the device took its last object and parked) — reset to re-arm it",
    3: "a commit was refused and its extents are held until a remount — reset before retrying",
    4: "bytes arrived that the device could not frame, so it refused the measurement run — something "
    "else is on this tty (a screen/minicom at another rate, a second talker, a failing cable). Note "
    "this is NOT a --baud mismatch: at the wrong baud this host never transmits at all",
}


class IngestError(RuntimeError):
    """The transfer did not complete. The device has cancelled whatever it held."""


class RetriableError(IngestError):
    """A failure the device cleaned up after, so sending the object again is worth doing."""


class LinkTimeout(RetriableError):
    pass


class DeviceGone(IngestError):
    """The device said it has stopped listening, and why. Retrying will not help until it is reset."""

    def __init__(self, reason: int):
        self.reason = reason
        super().__init__(f"the device has stopped listening: {GONE_REASONS.get(reason, f'reason {reason}')}")


def crc32(data: bytes) -> int:
    """CRC-32/IEEE, the one `obc-crc` implements. `crc32(b"123456789") == 0xCBF43926`."""
    return zlib.crc32(data) & 0xFFFF_FFFF


def header_frame(kind: int, name: str, payload: bytes) -> bytes:
    """The 72-byte HEADER for one object."""
    encoded = name.encode("utf-8")
    if len(encoded) > NAME_CAPACITY:
        raise IngestError(f"the name is {len(encoded)} bytes of UTF-8; the store's cap is {NAME_CAPACITY}")
    if not payload:
        raise IngestError("refusing to send a zero-length payload")
    frame = bytearray(HEADER_BYTES)
    frame[0:4] = MAGIC
    frame[4] = VERSION
    frame[5] = TAG_HEADER
    frame[6] = kind
    frame[7] = len(encoded)
    frame[8:16] = len(payload).to_bytes(8, "little")
    frame[16:20] = crc32(payload).to_bytes(4, "little")
    frame[20 : 20 + len(encoded)] = encoded
    frame[68:72] = crc32(bytes(frame[:68])).to_bytes(4, "little")
    return bytes(frame)


def ready_frame(chunk: int) -> bytes:
    """The device's 14-byte READY. Here so the tests can build one; the device is what sends it."""
    frame = bytearray(READY_BYTES)
    frame[0:4] = MAGIC
    frame[4] = VERSION
    frame[5] = TAG_READY
    frame[6:10] = chunk.to_bytes(4, "little")
    frame[10:14] = crc32(bytes(frame[:10])).to_bytes(4, "little")
    return bytes(frame)


def parse_short(frame: bytes) -> tuple[int, int]:
    """A 14-byte READY or GONE, as `(tag, value)`. They share a shape so one reader decodes both."""
    if len(frame) != READY_BYTES or frame[:4] != MAGIC:
        raise IngestError("not a 14-byte framed message")
    if frame[4] != VERSION:
        raise IngestError(f"the device speaks wire version {frame[4]}, this host speaks {VERSION}")
    if int.from_bytes(frame[10:14], "little") != crc32(frame[:10]):
        raise IngestError("the frame's CRC did not check")
    return frame[5], int.from_bytes(frame[6:10], "little")


def parse_ready(frame: bytes) -> int:
    """The chunk size a READY advertises."""
    tag, chunk = parse_short(frame)
    if tag == TAG_GONE:
        raise DeviceGone(chunk)
    if tag != TAG_READY:
        raise IngestError(f"expected a READY frame, got tag {tag:#04x}")
    if not 0 < chunk <= 1 << 20:
        raise IngestError(f"the device advertised an implausible chunk size of {chunk}")
    return chunk


@dataclass(frozen=True)
class Result:
    """What the device's RESULT frame said."""

    ok: bool
    reason: int
    object_id: int
    revision: int
    payload_len: int
    payload_crc: int
    entries: int

    def describe(self) -> str:
        if self.ok:
            return (
                f"object {self.object_id} revision {self.revision}: {self.payload_len:,} B, "
                f"crc {self.payload_crc:#010x} — the catalog now holds {self.entries} entries"
            )
        return f"the device refused the object: {REASONS.get(self.reason, f'reason {self.reason}')}"


def parse_result(frame: bytes) -> Result:
    """The 42-byte RESULT that closes an object out."""
    if len(frame) != RESULT_BYTES or frame[:4] != MAGIC:
        raise IngestError("not a RESULT frame")
    if frame[4] != VERSION:
        raise IngestError(f"the device speaks wire version {frame[4]}, this host speaks {VERSION}")
    if int.from_bytes(frame[38:42], "little") != crc32(frame[:38]):
        raise IngestError("the RESULT frame's CRC did not check")
    if frame[5] not in (TAG_DONE, TAG_FAIL):
        raise IngestError(f"expected a RESULT frame, got tag {frame[5]:#04x}")
    return Result(
        ok=frame[5] == TAG_DONE,
        reason=frame[6],
        object_id=int.from_bytes(frame[8:16], "little"),
        revision=int.from_bytes(frame[16:24], "little"),
        payload_len=int.from_bytes(frame[24:32], "little"),
        payload_crc=int.from_bytes(frame[32:36], "little"),
        entries=int.from_bytes(frame[36:38], "little"),
    )


def chunk_plan(total: int, chunk: int) -> list[int]:
    """The chunk lengths both sides compute, which is why no chunk carries a length field."""
    if chunk <= 0:
        raise IngestError("the chunk size must be positive")
    whole, remainder = divmod(total, chunk)
    plan = [chunk] * whole
    if remainder:
        plan.append(remainder)
    return plan


# ── the link ────────────────────────────────────────────────────────────────────────────────────


class Link:
    """A byte pipe to the device. `SerialLink` is the real one; the tests supply a fake."""

    def write(self, data: bytes) -> None:
        raise NotImplementedError

    def read_exact(self, count: int, timeout: float) -> bytes:
        """Exactly `count` bytes, or `LinkTimeout`."""
        raise NotImplementedError


class SerialLink(Link):
    def __init__(self, port: str, baud: int):
        try:
            import serial  # noqa: PLC0415 — pyserial is only needed for a real transfer
        except ImportError as error:  # pragma: no cover — environment, not logic
            raise IngestError(
                "pyserial is not installed. `uv venv && uv pip install pyserial`, then run this "
                "with that venv's python (the repo's .venv has no pip)."
            ) from error
        try:
            self._port = serial.Serial(port, baud, timeout=0.05, rtscts=False, dsrdtr=False)
        except OSError as error:  # pragma: no cover — environment, not logic
            # `serial.SerialException` is an OSError. A traceback here tells an operator nothing they
            # can act on; the two things that are actually wrong are the tty name and the cable.
            raise IngestError(
                f"cannot open {port}: {error}. On macOS the DK exposes two CDC ttys and only one is "
                "live — try the other (`ls /dev/cu.usbmodem*`), use `cu.*` rather than `tty.*`, and "
                "check nothing else (a `screen`, another run of this) is holding the port."
            ) from error

    def write(self, data: bytes) -> None:
        self._port.write(data)
        self._port.flush()

    def read_exact(self, count: int, timeout: float) -> bytes:
        deadline = time.monotonic() + timeout
        got = bytearray()
        while len(got) < count:
            got += self._port.read(count - len(got))
            if len(got) < count and time.monotonic() >= deadline:
                raise LinkTimeout(f"wanted {count} bytes, got {len(got)} in {timeout:.1f} s")
        return bytes(got)

    def close(self) -> None:
        self._port.close()


# ── the transfer ────────────────────────────────────────────────────────────────────────────────


def await_ready(link: Link, wait: float) -> int:
    """Scan the line for the device's READY advertisement and return its chunk size.

    The magic is matched a byte at a time because the line may already carry the tail of an older
    frame — a RESULT from a previous attempt, most likely — and a re-run must be able to pick the
    conversation back up rather than insisting the device was rebooted for it.

    A read that comes back empty is **not** a failure here: the device advertises about twice a
    second, so quiet is the normal state of this line and only the outer deadline ends the wait. A
    frame that arrives damaged is not a failure either — the next advertisement is 500 ms away, and
    giving up on one bad CRC would turn a single glitched byte into "the board is not there".

    A `GONE` frame is the one thing that ends the wait early, and it is the whole reason this does
    not have to guess: the device says which of four unrelated things happened, one of which is "the
    destructive measurement run is starting right now".
    """
    deadline = time.monotonic() + wait
    matched = 0
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise LinkTimeout(
                f"no frame of any kind in {wait:.0f} s — not even the GONE the device sends on every "
                "exit path. In order of likelihood:\n"
                "  1. --baud disagrees with INGEST_BAUD in flat_store_bench.rs. This is the one to "
                "check first, and its signature is RTT saying `nobody answered` while this waits: "
                "nothing decodes here, so this host never transmits, so the device sees an idle "
                "line and runs the DESTRUCTIVE measurement suite. Nothing errors on either side.\n"
                "  2. the bench is not running — start it with `cargo run --release --bin "
                "flat_store_bench`.\n"
                "  3. wrong tty — the DK exposes two and only one is live (`ls /dev/cu.usbmodem*`).\n"
                "  4. the J-Link's VCOM has wedged: writes succeed, RTT keeps flowing, nothing "
                "reaches the device. Only a physical power-cycle of the DK clears it (`probe-rs "
                "reset` does not). Suspect this only once 1-3 are ruled out."
            )
        try:
            byte = link.read_exact(1, min(remaining, 1.0))[0]
        except LinkTimeout:
            matched = 0
            continue
        if byte == MAGIC[matched]:
            matched += 1
        elif byte == MAGIC[0]:
            matched = 1
        else:
            matched = 0
        if matched == len(MAGIC):
            matched = 0
            try:
                rest = link.read_exact(READY_BYTES - len(MAGIC), min(max(remaining, 0.1), 2.0))
            except LinkTimeout:
                continue  # a truncated frame; the next advertisement is 500 ms away
            frame = MAGIC + rest
            if frame[5] not in (TAG_READY, TAG_GONE):
                continue  # a stale RESULT or STATUS from an earlier attempt; keep looking
            try:
                return parse_ready(frame)
            except DeviceGone:
                raise  # the device told us why; that answer is not improved by waiting
            except IngestError:
                continue  # a damaged advertisement; another is 500 ms away


def expect_ack(link: Link, timeout: float, what: str) -> None:
    """One STATUS byte pair.

    A NAK whose reason is in `RETRIABLE` is raised as a `RetriableError`, not a plain one: the most
    likely mid-transfer failure on this cable is the device's own chunk deadline expiring (reason
    11), and treating that as fatal would mean a manual re-run that has to land inside the device's
    ten-second inter-object window. That is the difference between `--attempts` meaning what its
    help text says and meaning nothing.
    """
    status = link.read_exact(2, timeout)
    if status[0] == STATUS_ACK:
        return
    if status[0] == STATUS_NAK:
        reason = status[1]
        message = f"the device refused {what}: {REASONS.get(reason, f'reason {reason}')}"
        raise (RetriableError if reason in RETRIABLE else IngestError)(message)
    raise IngestError(f"the device answered {what} with {status[0]:#04x}, which is not a status byte")


def send(
    link: Link,
    kind: int,
    name: str,
    payload: bytes,
    *,
    wait: float = 60.0,
    timeout: float = 30.0,
    progress=None,
) -> Result:
    """One object, handshake to RESULT. Raises `IngestError` if it did not get there.

    **The input buffer is deliberately not flushed first.** An earlier version cleared it to be tidy,
    which threw away the one frame this tool most needs to see: the device sends `GONE` once, and a
    re-run started after that would discard it and then wait out its whole timeout blaming the
    cable. `await_ready`'s magic scan already skips stale bytes correctly, so there is nothing for a
    flush to fix and a real diagnostic to lose.
    """
    chunk = await_ready(link, wait)
    link.write(header_frame(kind, name, payload))
    expect_ack(link, timeout, "the header")

    sent = 0
    for length in chunk_plan(len(payload), chunk):
        link.write(payload[sent : sent + length])
        expect_ack(link, timeout, f"the chunk at {sent:,} B")
        sent += length
        if progress is not None:
            progress(sent, len(payload))
    return parse_result(link.read_exact(RESULT_BYTES, timeout))


# ── the command line ────────────────────────────────────────────────────────────────────────────


def _progress(started: float):
    def report(done: int, total: int) -> None:
        elapsed = max(time.monotonic() - started, 1e-6)
        rate = done / elapsed
        remaining = (total - done) / rate if rate > 0 else 0.0
        line = f"  {done:,} / {total:,} B  ({100 * done / total:5.1f}%)  {rate / 1024:6.1f} kB/s  {remaining:5.1f} s left"
        end = "\r" if sys.stderr.isatty() and done < total else "\n"
        print(line, end=end, file=sys.stderr, flush=True)

    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="bench_ingest",
        description="Send a file into a board's flat store over the DK's VCOM UART (flat_store_bench's ingest mode).",
    )
    parser.add_argument("--port", required=True, help="the VCOM tty, e.g. /dev/cu.usbmodem*133")
    parser.add_argument("--file", required=True, type=Path, help="the payload to publish")
    parser.add_argument("--kind", default="map", choices=sorted(KINDS), help="FLAT_Store_Format.md §3.1 object kind")
    parser.add_argument("--name", help="the store's display name (48 bytes of UTF-8); defaults to the file's name")
    parser.add_argument(
        "--baud",
        type=int,
        default=115200,
        help="must match INGEST_BAUD in flat_store_bench.rs. A mismatch fails SILENTLY on both "
        "sides — this host never decodes a READY so it never transmits, and the device sees an idle "
        "line and runs its destructive measurement suite (default: 115200)",
    )
    parser.add_argument(
        "--wait",
        type=float,
        default=60.0,
        help="seconds to wait for the device on the FIRST attempt — generous, because the intended "
        "order is to start this and then flash the board (default: 60)",
    )
    parser.add_argument("--timeout", type=float, default=30.0, help="seconds to wait for any one ack")
    parser.add_argument(
        "--attempts",
        type=int,
        default=2,
        help="retries after a failure the device cleaned up after — a link timeout or a payload-CRC "
        "mismatch. The failed attempt was cancelled, so a retry is a fresh put with a new ObjectId "
        "and never a half-written object (default: 2)",
    )
    args = parser.parse_args(argv)

    try:
        payload = args.file.read_bytes()
    except OSError as error:
        print(f"cannot read {args.file}: {error.strerror}", file=sys.stderr)
        return 2
    name = args.name or os.path.basename(args.file)
    kind = KINDS[args.kind]
    print(
        f"{args.file}: {len(payload):,} B, crc {crc32(payload):#010x} — sending as {args.kind} "
        f"named {name!r} at {args.baud} baud",
        file=sys.stderr,
    )

    try:
        link = SerialLink(args.port, args.baud)
    except IngestError as error:
        print(f"{error}", file=sys.stderr)
        return 2

    try:
        for attempt in range(1, args.attempts + 1):
            # The first attempt waits for a human to flash the board; a retry is racing the device's
            # ten-second inter-object window, and waiting a minute for a board that stopped listening
            # forty seconds ago helps nobody.
            wait = args.wait if attempt == 1 else min(args.wait, RETRY_WAIT)
            try:
                print(f"waiting for the device (attempt {attempt} of {args.attempts})…", file=sys.stderr)
                started = time.monotonic()
                result = send(link, kind, name, payload, wait=wait, timeout=args.timeout, progress=_progress(started))
            except DeviceGone as error:
                print(f"{error}", file=sys.stderr)
                return 1
            except RetriableError as error:
                print(f"attempt {attempt} failed: {error}", file=sys.stderr)
                if attempt == args.attempts:
                    return 1
                print("retrying — the device cancelled that attempt and is advertising again", file=sys.stderr)
                continue
            except IngestError as error:
                print(f"{error}", file=sys.stderr)
                return 1
            print(result.describe(), file=sys.stderr)
            if result.ok:
                if result.payload_crc != crc32(payload):
                    print("the device committed a CRC this host did not send — investigate", file=sys.stderr)
                    return 1
                print(f"done in {time.monotonic() - started:.1f} s", file=sys.stderr)
                return 0
            if result.reason == REASON_COMMIT:
                # The device has ended its session and is holding those extents until a remount.
                print("reset the board before retrying — the reservation is freed by the mount", file=sys.stderr)
                return 1
            if result.reason in RETRIABLE and attempt < args.attempts:
                print("retrying — the device cancelled that attempt and is advertising again", file=sys.stderr)
                continue
            return 1
        return 1
    except KeyboardInterrupt:
        print("\ninterrupted — the device will cancel the attempt and go back to advertising", file=sys.stderr)
        return 130
    finally:
        link.close()


if __name__ == "__main__":
    raise SystemExit(main())
