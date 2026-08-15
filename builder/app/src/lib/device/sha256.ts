/**
 * Incremental SHA-256 (FIPS 180-4).
 *
 * Firmware release manifests are authenticated before an update is offered, but WebCrypto has
 * no incremental digest API. This implementation lets a response be verified as its chunks
 * arrive and is held to WebCrypto itself in `sha256.test.ts`.
/** Round constants: the first 32 bits of the fractional parts of the cube roots of the first 64 primes. */
// prettier-ignore
const K = new Uint32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

const rotr = (x: number, n: number): number => (x >>> n) | (x << (32 - n));

/**
 * A running SHA-256.
 *
 * Feed it any slicing of the message — the block boundaries are this class's problem, not the
 * caller's, which is what makes it usable from a stream reader that hands over whatever the network
 * delivered.
 */
export class Sha256 {
    private readonly h = new Uint32Array([
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ]);
    /** The partial block carried between updates, and how much of it is filled. */
    private readonly block = new Uint8Array(64);
    private blockLen = 0;
    /** Total message length in **bits**, as the padding needs it. Kept as a float: a `number` is
     *  exact to 2^53 bits ≈ 1 PB, comfortably past any artifact. */
    private bitLen = 0;
    private readonly w = new Uint32Array(64);
    private done = false;

    /** Fold in the next slice of the message. */
    update(bytes: Uint8Array): this {
        if (this.done) throw new Error("Sha256.update after digest()");
        this.bitLen += bytes.length * 8;
        let at = 0;
        // Top up a partial block first, so the fast path below can work straight off the input.
        if (this.blockLen > 0) {
            const take = Math.min(64 - this.blockLen, bytes.length);
            this.block.set(bytes.subarray(0, take), this.blockLen);
            this.blockLen += take;
            at = take;
            if (this.blockLen < 64) return this;
            this.compress(this.block, 0);
            this.blockLen = 0;
        }
        for (; at + 64 <= bytes.length; at += 64) this.compress(bytes, at);
        if (at < bytes.length) {
            this.block.set(bytes.subarray(at), 0);
            this.blockLen = bytes.length - at;
        }
        return this;
    }

    /** Finish the message and return the 32-byte digest. The instance is spent afterwards. */
    digest(): Uint8Array {
        if (this.done) throw new Error("Sha256.digest called twice");
        this.done = true;
        const bits = this.bitLen;
        // 0x80, then zeroes, then the 64-bit big-endian bit length in the last 8 bytes.
        this.block[this.blockLen++] = 0x80;
        if (this.blockLen > 56) {
            this.block.fill(0, this.blockLen);
            this.compress(this.block, 0);
            this.blockLen = 0;
        }
        this.block.fill(0, this.blockLen);
        const tail = new DataView(this.block.buffer);
        // Split rather than `setBigUint64`: the high word is the bit count above 2^32, and going
        // through BigInt for a value that is always small would cost an allocation per digest.
        tail.setUint32(56, Math.floor(bits / 0x100000000), false);
        tail.setUint32(60, bits >>> 0, false);
        this.compress(this.block, 0);

        const out = new Uint8Array(32);
        const view = new DataView(out.buffer);
        for (let i = 0; i < 8; i++) view.setUint32(i * 4, this.h[i], false);
        return out;
    }

    /** The digest as lowercase hex — the spelling `OBCC_Spec.md` §9 uses. */
    hex(): string {
        return toHex(this.digest());
    }

    private compress(data: Uint8Array, offset: number): void {
        const w = this.w;
        for (let i = 0; i < 16; i++) {
            const at = offset + i * 4;
            w[i] = (data[at] << 24) | (data[at + 1] << 16) | (data[at + 2] << 8) | data[at + 3];
        }
        for (let i = 16; i < 64; i++) {
            const a = w[i - 15];
            const b = w[i - 2];
            const s0 = rotr(a, 7) ^ rotr(a, 18) ^ (a >>> 3);
            const s1 = rotr(b, 17) ^ rotr(b, 19) ^ (b >>> 10);
            w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
        }
        let [a, b, c, d, e, f, g, h] = this.h;
        for (let i = 0; i < 64; i++) {
            const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
            const ch = (e & f) ^ (~e & g);
            const t1 = (h + s1 + ch + K[i] + w[i]) | 0;
            const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
            const maj = (a & b) ^ (a & c) ^ (b & c);
            const t2 = (s0 + maj) | 0;
            h = g;
            g = f;
            f = e;
            e = (d + t1) | 0;
            d = c;
            c = b;
            b = a;
            a = (t1 + t2) | 0;
        }
        const h32 = this.h;
        h32[0] = (h32[0] + a) | 0;
        h32[1] = (h32[1] + b) | 0;
        h32[2] = (h32[2] + c) | 0;
        h32[3] = (h32[3] + d) | 0;
        h32[4] = (h32[4] + e) | 0;
        h32[5] = (h32[5] + f) | 0;
        h32[6] = (h32[6] + g) | 0;
        h32[7] = (h32[7] + h) | 0;
    }

    /** One-shot convenience, for the small inputs where streaming buys nothing. */
    static hex(bytes: Uint8Array): string {
        return new Sha256().update(bytes).hex();
    }
}

function toHex(bytes: Uint8Array): string {
    let out = "";
    for (const b of bytes) out += b.toString(16).padStart(2, "0");
    return out;
}
