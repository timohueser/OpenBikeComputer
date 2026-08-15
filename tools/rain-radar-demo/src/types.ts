export interface RainManifest {
  version: number;
  generation: string;
  generated_at: string;
  reference_time: string;
  key_prefix: string;
  previous_generations: string[];
  lattice: Lattice;
  cadence: {
    frame_step_min: number;
    frames: number;
    max_source_skew_s: number;
  };
  freshness: {
    manifest_max_age_s: number;
    next_generation_expected_at: string;
    stale_after: string;
  };
  attribution: Attribution[];
  frames: RainFrame[];
}

export interface Lattice {
  south_lat_udeg: number;
  west_lon_udeg: number;
  cell_udeg: number;
  width: number;
  height: number;
  shard_width: number;
  shard_height: number;
  shard_cols: number;
  shard_rows: number;
  tile_edge: number;
  entries_per_page: number;
  cell_size_m: number;
  covered_rows: { start: number; end: number };
}

export interface Attribution {
  source_id: string;
  text: string;
  url: string;
}

export interface RainFrame {
  offset_min: number;
  valid_at: string;
  present: string;
  shards: ManifestShard[];
}

export interface ManifestShard {
  col: number;
  row: number;
  bytes: number;
  object_crc32: string;
  observed: boolean;
}

export interface ProxyStats {
  weatherBase: string;
  maxRequests: number;
  upstreamRequests: number;
  cacheHits: number;
  coalescedHits: number;
  upstreamBytes: number;
  errors: number;
  inFlight: number;
  cacheEntries: number;
  cacheBytes: number;
}

export interface ShardId {
  col: number;
  row: number;
}
