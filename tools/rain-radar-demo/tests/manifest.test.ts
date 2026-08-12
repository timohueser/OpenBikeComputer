import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, test } from "vitest";
import { shardKey, shardsForBounds, validateManifest } from "../src/manifest";
import type { RainManifest } from "../src/types";

let manifest: RainManifest;
beforeAll(async () => {
  const path = fileURLToPath(new URL("../../../specs/vectors/wx-manifest-v2.json", import.meta.url));
  manifest = validateManifest(JSON.parse(await readFile(path, "utf8")));
});

describe("manifest viewport planning", () => {
  test("uses the manifest geometry and composes canonical object keys", () => {
    const shards = shardsForBounds(manifest.lattice, 47.8, 7.7, 48.1, 8.2);
    expect(shards).toEqual([{ col: 3, row: 2 }]);
    expect(shardKey(manifest.key_prefix, manifest.generation, 0, shards[0]))
      .toBe("wx/v2/20260810T1430Z/f0/s3-2.obcg");
  });

  test("selects only shards intersecting the visible map", () => {
    expect(shardsForBounds(manifest.lattice, -85, -180, 85, 180)).toHaveLength(24);
    expect(shardsForBounds(manifest.lattice, 10, -20, 20, 80)).toHaveLength(3);
  });
});
