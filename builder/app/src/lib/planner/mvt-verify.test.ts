import { expect, it } from 'vitest';
import { VectorTile } from '@mapbox/vector-tile';
import { PbfReader, PbfWriter } from 'pbf';

it('the publication reader accepts empty tiles and rejects malformed framing and lazy geometry', async () => {
    const { validateTile } = await import(new URL('../../../../../tools/planner_mvt_verify.mjs', import.meta.url).href);
    const readers = { VectorTile, PbfReader };
    expect(() => validateTile(new Uint8Array(), readers)).not.toThrow();
    expect(() => validateTile(new Uint8Array([0x80]), readers)).toThrow();
    const writer = new PbfWriter();
    writer.writeMessage(3, (_, layer) => {
        layer.writeStringField(1, 'pois');
        layer.writeVarintField(15, 2);
        layer.writeMessage(2, (_, feature) => {
            feature.writeVarintField(3, 1);
            feature.writeBytesField(4, new Uint8Array([0x80]));
        }, null);
    }, null);
    const lazy = writer.finish();
    expect(() => new VectorTile(new PbfReader(lazy))).not.toThrow();
    expect(() => validateTile(lazy, readers)).toThrow();
});
