"""Measure lossless storage candidates; this is not a device codec or timing benchmark."""

import json
import zlib
from pathlib import Path


def pack6(pixels):
    output = bytearray()
    bits = value = 0
    for pixel in pixels:
        assert pixel < 64
        value = (value << 6) | pixel
        bits += 6
        while bits >= 8:
            bits -= 8
            output.append((value >> bits) & 255)
        value &= (1 << bits) - 1
    if bits:
        output.append(value << (8 - bits))
    return bytes(output)


def unpack6(data, length):
    output = bytearray()
    bits = value = 0
    for byte in data:
        value = (value << 8) | byte
        bits += 8
        while bits >= 6 and len(output) < length:
            bits -= 6
            output.append((value >> bits) & 63)
        value &= (1 << bits) - 1
    return bytes(output)


def main():
    root = Path(__file__).resolve().parents[4]
    assets = sorted((root / "firmware/obc-app/assets/landmarks").glob("*-large.rgb222"))
    assets += sorted((root / "apps/obc-sim/assets/landmarks").glob("*.rgb222"))
    results = {}
    for path in assets:
        pixels = path.read_bytes()
        assert len(pixels) == 216 * 240
        packed = pack6(pixels)
        assert unpack6(packed, len(pixels)) == pixels
        compressed = zlib.compress(pixels, 6)
        compressed_packed = zlib.compress(packed, 6)
        assert zlib.decompress(compressed) == pixels
        assert unpack6(zlib.decompress(compressed_packed), len(pixels)) == pixels
        # Store only the non-white rectangle, with four uint16 bounds (8 bytes).
        points = [(i % 216, i // 216) for i, p in enumerate(pixels) if p != 63]
        left, right = min(x for x, _ in points), max(x for x, _ in points) + 1
        top, bottom = min(y for _, y in points), max(y for _, y in points) + 1
        rectangle = b"".join(pixels[y * 216 + left:y * 216 + right] for y in range(top, bottom))
        restored = bytearray([63] * len(pixels))
        for row, y in enumerate(range(top, bottom)):
            restored[y * 216 + left:y * 216 + right] = rectangle[row * (right-left):(row+1) * (right-left)]
        assert restored == pixels
        results[path.stem] = {
            "raw": len(pixels),
            "packed6": len(packed),
            "packed6_rectangle_with_bounds": len(pack6(rectangle)) + 8,
            "zlib6_raw": len(compressed),
            "zlib6_packed6": len(compressed_packed),
        }
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
