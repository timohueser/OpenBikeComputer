import type { Lattice, RainManifest, ShardId } from "./types";

export function validateManifest(value: unknown): RainManifest {
  const manifest = value as RainManifest;
  if (!manifest || manifest.version !== 2 || !Array.isArray(manifest.frames) || !manifest.lattice) {
    throw new Error("Weather service returned an unsupported manifest");
  }
  const grid = manifest.lattice;
  if (
    !Number.isInteger(grid.shard_cols) ||
    !Number.isInteger(grid.shard_rows) ||
    grid.shard_cols !== Math.ceil(grid.width / grid.shard_width) ||
    grid.shard_rows !== Math.ceil(grid.height / grid.shard_height)
  ) {
    throw new Error("Weather manifest has inconsistent shard geometry");
  }
  if (manifest.frames.length !== manifest.cadence.frames || manifest.frames.some((frame) => !Array.isArray(frame.shards))) {
    throw new Error("Weather manifest has an inconsistent timeline");
  }
  return manifest;
}

export function shardKey(prefix: string, generation: string, offsetMin: number, shard: ShardId): string {
  return `${prefix}/${generation}/f${offsetMin}/s${shard.col}-${shard.row}.obcg`;
}

export function shardsForBounds(
  grid: Lattice,
  southDeg: number,
  westDeg: number,
  northDeg: number,
  eastDeg: number,
): ShardId[] {
  const south = Math.max(southDeg * 1e6, grid.south_lat_udeg);
  const north = Math.min(northDeg * 1e6, grid.south_lat_udeg + grid.height * grid.cell_udeg);
  const west = Math.max(westDeg * 1e6, grid.west_lon_udeg);
  const east = Math.min(eastDeg * 1e6, grid.west_lon_udeg + grid.width * grid.cell_udeg);
  if (!(south < north && west < east)) return [];
  const firstCellRow = Math.max(0, Math.floor((south - grid.south_lat_udeg) / grid.cell_udeg));
  const lastCellRow = Math.min(grid.height - 1, Math.ceil((north - grid.south_lat_udeg) / grid.cell_udeg) - 1);
  const firstCellCol = Math.max(0, Math.floor((west - grid.west_lon_udeg) / grid.cell_udeg));
  const lastCellCol = Math.min(grid.width - 1, Math.ceil((east - grid.west_lon_udeg) / grid.cell_udeg) - 1);
  const shards: ShardId[] = [];
  for (let row = Math.floor(firstCellRow / grid.shard_height); row <= Math.floor(lastCellRow / grid.shard_height); row++) {
    for (let col = Math.floor(firstCellCol / grid.shard_width); col <= Math.floor(lastCellCol / grid.shard_width); col++) {
      shards.push({ col, row });
    }
  }
  return shards;
}
