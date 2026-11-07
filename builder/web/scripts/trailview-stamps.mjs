// The stamp process of the `obc data` step `planner/trailview-stamps` (host/obc-trailview-features/src/stamps.rs).
// All numbers are little-endian. It writes the stamp version first: a u32 length and its UTF-8. Then it reads
// feature tiles until standard input ends, each a u8 zoom, u32 x, u32 y, u32 length and the tile body, and writes
// for each the u32 count of its baked nodes, each a u8 level, u32 x, u32 y, u32 length and the node bytes.
//
//   node builder/web/scripts/trailview-stamps.mjs < tiles > nodes
import { stampBundle } from '../vite/stamp-bundle.ts';

const { code, version } = await stampBundle();
// The stamp worker sets `self.onmessage`.
globalThis.self = globalThis;
const { bakeTile } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);

const write = chunk => new Promise((resolve, reject) => process.stdout.write(chunk, error => (error ? reject(error) : resolve())));
const u32 = value => {
    const out = Buffer.alloc(4);
    out.writeUInt32LE(value);
    return out;
};
const place = (z, x, y, length) => {
    const out = Buffer.alloc(13);
    out.writeUInt8(z, 0);
    out.writeUInt32LE(x, 1);
    out.writeUInt32LE(y, 5);
    out.writeUInt32LE(length, 9);
    return out;
};

/** The next `n` bytes of standard input; null at its end where `last` allows it. */
const input = process.stdin[Symbol.asyncIterator]();
let held = Buffer.alloc(0);
async function read(n, last = false) {
    while (held.length < n) {
        const { value, done } = await input.next();
        if (done) {
            if (last && !held.length) return null;
            throw new Error(`standard input ends inside a tile: ${held.length} of ${n} bytes`);
        }
        held = Buffer.concat([held, value]);
    }
    const out = held.subarray(0, n);
    held = held.subarray(n);
    return out;
}

await write(Buffer.concat([u32(Buffer.byteLength(version)), Buffer.from(version)]));
for (let head; (head = await read(13, true)); ) {
    const tile = { level: head.readUInt8(0), x: head.readUInt32LE(1), y: head.readUInt32LE(5) };
    const body = await read(head.readUInt32LE(9));
    const nodes = await bakeTile(tile, body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength));
    await write(Buffer.concat([u32(nodes.length), ...nodes.flatMap(([node, bytes]) => [place(node.level, node.x, node.y, bytes.byteLength), bytes])]));
}
