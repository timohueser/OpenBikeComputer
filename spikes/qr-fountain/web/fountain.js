// Fountain-coded frames for an animated QR code.
//
// The payload is cut into K blocks of `blockSize` bytes (the last one zero-padded).
// Frame `seq < K` carries block `seq`. Every later frame carries the XOR of a random
// subset of blocks; the subset comes from a hash of (seq, crc), so the receiver
// rebuilds it without it being sent. Any K independent frames decode, so
// the order and the missed frames do not matter.
//
// Frame bytes (big-endian): u32 crc32(payload) | u32 length | u16 blockSize | u32 seq | block.
// The bytes go into the QR code as base45 (RFC 9285) in alphanumeric mode, because
// iOS only hands back the text of a QR code, not its raw bytes.
// ios/QRFountain/Fountain.swift is the receiver and must stay bit-identical;
// check/run.sh checks that.

export const HEADER_BYTES = 14;

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

export function crc32(bytes) {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

// murmur3's finalizer. The subset must come from a non-linear function: with a linear
// PRNG such as xorshift, all subsets span only as many dimensions as the seed has bits.
function fmix32(h) {
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
}

// Bit i of the result says whether block i is in frame `seq`.
export function coefficients(seq, blockCount, crc) {
  const words = new Uint32Array(Math.ceil(blockCount / 32));
  if (seq < blockCount) {
    words[seq >>> 5] = 1 << (seq & 31);
    return words;
  }
  const key = fmix32(seq ^ crc);
  for (let w = 0; w < words.length; w++) words[w] = fmix32((key + w) >>> 0);
  if (blockCount % 32) words[words.length - 1] &= (1 << (blockCount % 32)) - 1;
  if (words.every((w) => w === 0)) words[(seq % blockCount) >>> 5] = 1 << (seq % blockCount & 31);
  return words;
}

export class Encoder {
  constructor(payload, blockSize) {
    this.payload = payload;
    this.blockSize = blockSize;
    this.blockCount = Math.max(1, Math.ceil(payload.length / blockSize));
    this.crc = crc32(payload);
  }

  frame(seq) {
    const { payload, blockSize, blockCount, crc } = this;
    const out = new Uint8Array(HEADER_BYTES + blockSize);
    const view = new DataView(out.buffer);
    view.setUint32(0, crc);
    view.setUint32(4, payload.length);
    view.setUint16(8, blockSize);
    view.setUint32(10, seq);
    const words = coefficients(seq, blockCount, crc);
    for (let i = 0; i < blockCount; i++) {
      if (!(words[i >>> 5] & (1 << (i & 31)))) continue;
      const block = payload.subarray(i * blockSize, (i + 1) * blockSize);
      for (let j = 0; j < block.length; j++) out[HEADER_BYTES + j] ^= block[j];
    }
    return base45(out);
  }
}

const BASE45 = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:';

export function base45(bytes) {
  let s = '';
  for (let i = 0; i < bytes.length; i += 2) {
    if (i + 1 < bytes.length) {
      const n = bytes[i] * 256 + bytes[i + 1];
      s += BASE45[n % 45] + BASE45[Math.floor(n / 45) % 45] + BASE45[Math.floor(n / 2025)];
    } else {
      s += BASE45[bytes[i] % 45] + BASE45[Math.floor(bytes[i] / 45)];
    }
  }
  return s;
}
