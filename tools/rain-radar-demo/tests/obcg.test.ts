import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import { decodeTile, parseObcg, sampleTile } from "../src/obcg";

const vector = (name: string) => fileURLToPath(new URL(`../../../specs/vectors/${name}`, import.meta.url));
const load = async (name: string) => {
  const bytes = await readFile(vector(name));
  return Uint8Array.from(bytes).buffer;
};

describe("OBCG browser decoder", () => {
  test("recognizes the canonical dry sentinel without a payload", async () => {
    const object = parseObcg(await load("grid-minimal-dry.obcg"));
    expect(object.header.width).toBe(32);
    expect(object.header.height).toBe(32);
    expect(decodeTile(object, 0)).toBeNull();
  });

  test("decodes raw4, RLE4, and deflate4 vectors", async () => {
    const raw = parseObcg(await load("grid-raw-tile.obcg"));
    const rle = parseObcg(await load("grid-rle-tile.obcg"));
    const deflate = parseObcg(await load("grid-deflate-tile.obcg"));
    expect(new Set(decodeTile(raw, 0))).toContain(12);
    expect(decodeTile(rle, 0)).toEqual(new Uint8Array(256).fill(6));
    expect(decodeTile(deflate, 0)).toHaveLength(64 * 64);
    expect(new Set(decodeTile(deflate, 0)!).size).toBeGreaterThan(2);
  });

  test("keeps no-data distinct from a dry shard", async () => {
    const object = parseObcg(await load("grid-nodata-tile.obcg"));
    expect(decodeTile(object, 0)).toEqual(new Uint8Array(256).fill(15));
  });

  test("samples compressed tiles directly at screen resolution", async () => {
    const rle = parseObcg(await load("grid-rle-tile.obcg"));
    const deflate = parseObcg(await load("grid-deflate-tile.obcg"));
    expect(sampleTile(rle, 0, 3, 2)).toEqual(new Uint8Array(6).fill(6));
    expect(sampleTile(deflate, 0, 7, 5)).toHaveLength(35);
  });

  test("rejects corrupt object and directory integrity", async () => {
    await expect(async () => parseObcg(await load("grid-invalid-object-crc.obcg"))).rejects.toThrow("object CRC");
    await expect(async () => parseObcg(await load("grid-invalid-page-crc.obcg"))).rejects.toThrow("directory page CRC");
  });
});
