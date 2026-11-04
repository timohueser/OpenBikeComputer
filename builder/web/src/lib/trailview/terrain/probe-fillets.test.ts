import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { it } from 'vitest';
import { decodeFeatures } from '../stream/features';
import { features, filletsOf, metresPerUnit } from './bins';

const U = 2 ** 30;
const world = (lat: number, lon: number) => [((lon + 180) / 360) * U, ((1 - Math.log(Math.tan((lat * Math.PI) / 180) + 1 / Math.cos((lat * Math.PI) / 180)) / Math.PI) / 2) * U];
it('prints fillets near a place', () => {
    const [lat, lon] = (process.env.PLACE ?? '46.819890,8.407718').split(',').map(Number);
    const [x, y] = world(lat, lon), tile = { level: 13, x: Math.floor(x / 2 ** 17), y: Math.floor(y / 2 ** 17) };
    const list = decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/13/${tile.x}/${tile.y}.bin`, import.meta.url)))));
    const { lines } = features(list, tile, true), m = metresPerUnit(y);
    const f32 = (w: number) => new Float32Array(new Uint32Array([w]).buffer)[0];
    for (const { words } of filletsOf(lines)) {
        const [ox, oy, ax, ay] = words, bx = words[9], by = words[10];
        if (Math.hypot(ox - x, oy - y) * m > 40) continue;
        const r = (px: number, py: number) => `${((px - x) * m).toFixed(1)},${((py - y) * m).toFixed(1)}`;
        console.log(`O ${r(ox, oy)} A ${r(ax, ay)} B ${r(bx, by)} r ${f32(words[8]).toFixed(2)} |OA| ${(Math.hypot(ax - ox, ay - oy) * m).toFixed(2)} |OB| ${(Math.hypot(bx - ox, by - oy) * m).toFixed(2)} h ${f32(words[11]).toFixed(1)},${f32(words[12]).toFixed(1)} z ${f32(words[6]).toFixed(2)},${f32(words[7]).toFixed(2)} g ${f32(words[14]).toFixed(3)},${f32(words[15]).toFixed(3)}`);
    }
    for (const line of lines) {
        for (let v = 0; v < line.z.length; v++) {
            const d = Math.hypot(line.xy[2 * v] - x, line.xy[2 * v + 1] - y) * m;
            if (d < 25) console.log(`line kind ${line.kind} h ${line.h} v ${v}/${line.z.length} at ${((line.xy[2 * v] - x) * m).toFixed(1)},${((line.xy[2 * v + 1] - y) * m).toFixed(1)} anchor ${line.anchors?.[v]}`);
        }
    }
});
