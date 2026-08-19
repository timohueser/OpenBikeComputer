"""The serial-ingest wire, driven against a fake device.

`tools/bench_ingest.py` is the host half of a protocol whose other half only exists on a board, so
the framing, the chunk plan and the ack pacing have nowhere else to be checked. `FakeDevice` below
is a faithful-enough mirror of `flat_store_bench.rs`'s state machine — magic scan, header
validation, one ack per chunk, CRC before commit — which is what makes these tests say something
about the device rather than only about the host talking to itself.
"""

import unittest

from tools.bench_ingest import (
    HEADER_BYTES,
    KINDS,
    MAGIC,
    RESULT_BYTES,
    STATUS_ACK,
    STATUS_NAK,
    TAG_DONE,
    TAG_FAIL,
    TAG_HEADER,
    TAG_READY,
    VERSION,
    IngestError,
    Link,
    LinkTimeout,
    Result,
    await_ready,
    chunk_plan,
    crc32,
    header_frame,
    parse_ready,
    parse_result,
    ready_frame,
    send,
)


class FakeDevice(Link):
    """The device half of the wire, in Python.

    It advertises READY whenever it is idle and the host wants bytes it does not have, which is what
    the real one does on a timer. `corrupt_at` flips one byte of the payload *in flight*, so the
    device folds a CRC over bytes the host never sent — the failure the whole protocol exists to
    catch before a commit.
    """

    def __init__(self, chunk=4096, capacity=1 << 20, corrupt_at=None):
        self.chunk = chunk
        self.capacity = capacity
        self.corrupt_at = corrupt_at
        self.tx = bytearray()
        self.committed = []
        self.next_id = 1
        self.events = []
        self.refusals = []
        self._reset()

    def _reset(self):
        self.state = "magic"
        self.matched = 0
        self.frame = bytearray()
        self.payload = bytearray()
        self.plan = []
        self.received = 0

    # ── the Link face the host drives ──────────────────────────────────────────────────────────

    def write(self, data):
        for byte in data:
            self._byte(byte)

    def read_exact(self, count, timeout):
        if len(self.tx) < count and self.state == "magic":
            self.tx += ready_frame(self.chunk)
            self.events.append("ready")
        if len(self.tx) < count:
            raise LinkTimeout(f"the fake device has {len(self.tx)} bytes, not {count}")
        out, self.tx = bytes(self.tx[:count]), self.tx[count:]
        return out

    def drain(self):
        self.tx.clear()

    def close(self):
        pass

    # ── the state machine ──────────────────────────────────────────────────────────────────────

    def _byte(self, byte):
        if self.state == "magic":
            if byte == MAGIC[self.matched]:
                self.matched += 1
            elif byte == MAGIC[0]:
                self.matched = 1
            else:
                self.matched = 0
            if self.matched == len(MAGIC):
                self.matched = 0
                self.state = "header"
                self.frame = bytearray(MAGIC)
        elif self.state == "header":
            self.frame.append(byte)
            if len(self.frame) == HEADER_BYTES:
                self._header(bytes(self.frame))
        elif self.state == "payload":
            if self.corrupt_at == len(self.payload):
                byte ^= 0xFF
            self.payload.append(byte)
            self.received += 1
            if self.received == self.plan[0]:
                self.plan.pop(0)
                self.received = 0
                self._ack()
                if not self.plan:
                    self._finish()

    def _ack(self):
        self.tx += bytes([STATUS_ACK, 0])
        self.events.append("ack")

    def _nak(self, reason):
        self.tx += bytes([STATUS_NAK, reason])
        self.refusals.append(reason)
        self._reset()

    def _header(self, frame):
        if frame[4] != VERSION or frame[5] != TAG_HEADER:
            return self._nak(1)
        if int.from_bytes(frame[68:72], "little") != crc32(frame[:68]):
            return self._nak(2)
        if frame[6] not in KINDS.values():
            return self._nak(3)
        name_len = frame[7]
        if name_len > 48:
            return self._nak(4)
        try:
            self.name = frame[20 : 20 + name_len].decode("utf-8")
        except UnicodeDecodeError:
            return self._nak(4)
        self.length = int.from_bytes(frame[8:16], "little")
        self.want_crc = int.from_bytes(frame[16:20], "little")
        self.kind = frame[6]
        if self.length == 0:
            return self._nak(5)
        if self.length > self.capacity:
            return self._nak(7)
        self.plan = chunk_plan(self.length, self.chunk)
        self.payload = bytearray()
        self.received = 0
        self.state = "payload"
        self._ack()
        self.events.append("header")
        return None

    def _finish(self):
        got = crc32(bytes(self.payload))
        if got != self.want_crc:
            # The reservation is cancelled, so nothing is committed and no id is consumed.
            self._result(TAG_FAIL, 9, 0, 0, len(self.payload), got, 0)
        else:
            object_id = self.next_id
            self.next_id += 1
            self.committed.append((object_id, self.kind, self.name, bytes(self.payload)))
            self._result(TAG_DONE, 0, object_id, 1, self.length, got, len(self.committed))
        self._reset()

    def _result(self, tag, reason, object_id, revision, length, payload_crc, entries):
        frame = bytearray(RESULT_BYTES)
        frame[0:4] = MAGIC
        frame[4] = VERSION
        frame[5] = tag
        frame[6] = reason
        frame[8:16] = object_id.to_bytes(8, "little")
        frame[16:24] = revision.to_bytes(8, "little")
        frame[24:32] = length.to_bytes(8, "little")
        frame[32:36] = payload_crc.to_bytes(4, "little")
        frame[36:38] = entries.to_bytes(2, "little")
        frame[38:42] = crc32(bytes(frame[:38])).to_bytes(4, "little")
        self.tx += frame
        self.events.append("result")


class ScriptedLink(Link):
    """A link whose device side is a fixed script. For the parts of the handshake `send` skips."""

    def __init__(self, script: bytes):
        self.tx = bytearray(script)
        self.written = bytearray()

    def write(self, data):
        self.written += data

    def read_exact(self, count, timeout):
        if len(self.tx) < count:
            raise LinkTimeout("script exhausted")
        out, self.tx = bytes(self.tx[:count]), self.tx[count:]
        return out


class Crc(unittest.TestCase):
    def test_check_value(self):
        # The one `FLAT_Store_Format.md` names, and the one obc-crc is tested against.
        self.assertEqual(crc32(b"123456789"), 0xCBF43926)


class Frames(unittest.TestCase):
    def test_header_layout(self):
        payload = bytes(range(256)) * 4
        frame = header_frame(KINDS["map"], "monaco.obcm", payload)
        self.assertEqual(len(frame), HEADER_BYTES)
        self.assertEqual(frame[:4], MAGIC)
        self.assertEqual(frame[4], VERSION)
        self.assertEqual(frame[5], TAG_HEADER)
        self.assertEqual(frame[6], 5)
        self.assertEqual(frame[7], len("monaco.obcm"))
        self.assertEqual(int.from_bytes(frame[8:16], "little"), len(payload))
        self.assertEqual(int.from_bytes(frame[16:20], "little"), crc32(payload))
        self.assertEqual(frame[20:31], b"monaco.obcm")
        self.assertEqual(frame[31:68], bytes(37))  # the name is zero-padded to 48
        self.assertEqual(int.from_bytes(frame[68:72], "little"), crc32(frame[:68]))

    def test_header_refuses_an_oversized_name(self):
        with self.assertRaisesRegex(IngestError, "48"):
            header_frame(KINDS["map"], "n" * 49, b"x")

    def test_header_refuses_an_empty_payload(self):
        with self.assertRaisesRegex(IngestError, "zero-length"):
            header_frame(KINDS["map"], "empty", b"")

    def test_ready_round_trip(self):
        self.assertEqual(parse_ready(ready_frame(8192)), 8192)

    def test_ready_rejects_a_bad_crc(self):
        frame = bytearray(ready_frame(8192))
        frame[6] ^= 0x01
        with self.assertRaisesRegex(IngestError, "CRC"):
            parse_ready(bytes(frame))

    def test_ready_rejects_another_wire_version(self):
        frame = bytearray(ready_frame(8192))
        frame[4] = 2
        frame[10:14] = crc32(bytes(frame[:10])).to_bytes(4, "little")
        with self.assertRaisesRegex(IngestError, "version"):
            parse_ready(bytes(frame))

    def test_result_rejects_a_bad_crc(self):
        device = FakeDevice()
        device._result(TAG_DONE, 0, 7, 1, 100, 0xDEAD, 3)
        frame = bytearray(device.tx)
        frame[8] ^= 0x01
        with self.assertRaisesRegex(IngestError, "CRC"):
            parse_result(bytes(frame))

    def test_result_parses(self):
        device = FakeDevice()
        device._result(TAG_DONE, 0, 7, 1, 100, 0xDEAD, 3)
        self.assertEqual(
            parse_result(bytes(device.tx)),
            Result(ok=True, reason=0, object_id=7, revision=1, payload_len=100, payload_crc=0xDEAD, entries=3),
        )


class ChunkPlan(unittest.TestCase):
    def test_exact_multiple(self):
        self.assertEqual(chunk_plan(8192, 4096), [4096, 4096])

    def test_short_last_chunk(self):
        self.assertEqual(chunk_plan(9000, 4096), [4096, 4096, 808])

    def test_smaller_than_one_chunk(self):
        self.assertEqual(chunk_plan(7, 4096), [7])

    def test_a_plan_always_covers_the_payload(self):
        for total in (1, 4095, 4096, 4097, 123_456):
            self.assertEqual(sum(chunk_plan(total, 4096)), total)


class Handshake(unittest.TestCase):
    def test_skips_a_stale_frame_before_the_ready(self):
        # A RESULT left on the line by an earlier attempt must not be mistaken for a READY, and must
        # not stop the scan either — a re-run has to pick the conversation back up.
        device = FakeDevice()
        device._result(TAG_FAIL, 9, 0, 0, 10, 0, 0)
        link = ScriptedLink(b"\x00noise" + bytes(device.tx) + ready_frame(2048))
        self.assertEqual(await_ready(link, wait=5.0), 2048)

    def test_reports_the_wedge_when_nothing_arrives(self):
        with self.assertRaisesRegex(LinkTimeout, "power-cycle"):
            await_ready(ScriptedLink(b""), wait=0.05)


class Transfer(unittest.TestCase):
    def payload(self, length):
        return bytes((index * 7 + 11) & 0xFF for index in range(length))

    def test_publishes_the_payload_byte_for_byte(self):
        device = FakeDevice(chunk=1024)
        payload = self.payload(4096)
        result = send(device, KINDS["map"], "monaco.obcm", payload)
        self.assertTrue(result.ok)
        self.assertEqual(result.object_id, 1)
        self.assertEqual(result.revision, 1)
        self.assertEqual(result.payload_len, len(payload))
        self.assertEqual(result.payload_crc, crc32(payload))
        self.assertEqual(device.committed, [(1, KINDS["map"], "monaco.obcm", payload)])

    def test_a_short_last_chunk_arrives_whole(self):
        device = FakeDevice(chunk=1024)
        payload = self.payload(2561)
        result = send(device, KINDS["route"], "grimsel.obcr", payload)
        self.assertTrue(result.ok)
        self.assertEqual(device.committed[0][3], payload)

    def test_every_chunk_is_acked_before_the_next_one_is_sent(self):
        # The pacing *is* the flow control on this cable: one ack per chunk, and nothing in flight
        # while the device is writing. A run whose events interleave any other way has lost that.
        device = FakeDevice(chunk=1024)
        send(device, KINDS["map"], "map", self.payload(4096))
        self.assertEqual(
            device.events,
            ["ready", "ack", "header", "ack", "ack", "ack", "ack", "result"],
        )

    def test_a_corrupted_payload_is_never_committed(self):
        device = FakeDevice(chunk=1024, corrupt_at=2000)
        payload = self.payload(4096)
        result = send(device, KINDS["map"], "map", payload)
        self.assertFalse(result.ok)
        self.assertEqual(result.reason, 9)
        self.assertEqual(device.committed, [])

    def test_a_retry_after_a_failure_is_a_fresh_put(self):
        # The device cancelled the failed attempt, so the retry is a new reservation and a new
        # ObjectId — never a resumed half-written object.
        device = FakeDevice(chunk=1024, corrupt_at=2000)
        payload = self.payload(4096)
        self.assertFalse(send(device, KINDS["map"], "map", payload).ok)
        device.corrupt_at = None
        result = send(device, KINDS["map"], "map", payload)
        self.assertTrue(result.ok)
        self.assertEqual(result.object_id, 1, "a cancelled attempt must not consume an ObjectId")
        self.assertEqual(len(device.committed), 1)

    def test_two_objects_over_one_session(self):
        device = FakeDevice(chunk=1024)
        first = send(device, KINDS["map"], "map", self.payload(2048))
        second = send(device, KINDS["route"], "route", self.payload(600))
        self.assertEqual((first.object_id, second.object_id), (1, 2))
        self.assertEqual(second.entries, 2)

    def test_a_payload_the_card_cannot_hold_is_refused_before_any_chunk(self):
        device = FakeDevice(chunk=1024, capacity=1024)
        with self.assertRaisesRegex(IngestError, "free extents"):
            send(device, KINDS["map"], "map", self.payload(4096))
        self.assertEqual(device.refusals, [7])
        self.assertEqual(device.committed, [])

    def test_an_unknown_kind_is_refused(self):
        device = FakeDevice(chunk=1024)
        with self.assertRaisesRegex(IngestError, "§3.1"):
            send(device, 99, "map", self.payload(1024))
        self.assertEqual(device.refusals, [3])

    def test_progress_is_reported_once_per_chunk(self):
        device = FakeDevice(chunk=1024)
        seen = []
        send(device, KINDS["map"], "map", self.payload(3000), progress=lambda done, total: seen.append((done, total)))
        self.assertEqual(seen, [(1024, 3000), (2048, 3000), (3000, 3000)])


if __name__ == "__main__":
    unittest.main()
