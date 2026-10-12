// Writes a random payload and the frames a receiver catches when it misses `loss` of them.
// Usage: node frames.mjs OUT_DIR PAYLOAD_BYTES BLOCK_BYTES LOSS
import { writeFileSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { Encoder } from '../web/fountain.js';

const [dir, size, blockSize, loss] = process.argv.slice(2);
const payload = new Uint8Array(randomBytes(+size));
const encoder = new Encoder(payload, +blockSize);
const frames = [];
for (let seq = 0; frames.length < encoder.blockCount * 2; seq++) if (Math.random() >= +loss) frames.push(encoder.frame(seq));
writeFileSync(`${dir}/payload.bin`, payload);
writeFileSync(`${dir}/frames.txt`, frames.join('\n'));
