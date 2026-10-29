// How this machine holds each source: `held` of `obc data sources --json`, and what
// `obc data sources --region ID --json` writes.
import type { Bbox } from "./areas";
import { count, size } from "./shapes";
import { isReference, type SourceRow } from "./sources";

export type HeldStatus = "archived" | "prepared" | "held" | "partial" | "missing" | "live";

export interface Held {
    rule: "keep" | "prepare" | "store" | "live";
    covers: string;
    status: HeldStatus;
    where: string | null;
    dataset: string | null;
    /** The prepared source whose versions hold it: itself, or the one that reads it. */
    prepared_by: string | null;
    bytes: number | null;
    /** What an `archive get` still takes; `null` when unknown. */
    download_bytes: number | null;
    date: string | null;
    archive_files: { verified: number; listed: number } | null;
    store_versions: number;
    store_requests: number;
    /** A national terrain model that nothing holds: the terrain there is GLO-30 only. */
    gap: boolean;
    no_dataset: boolean;
    error: string | null;
}

/** The statuses in the order of the summary row. */
export const STATUSES: Record<HeldStatus, string> = {
    archived: "Archived",
    prepared: "Prepared",
    held: "Held",
    partial: "Partial",
    missing: "Missing",
    live: "Live",
};

export type Need = "held" | "download" | "prepare" | "unknown";

export const RULES: Record<Held["rule"], string> = {
    keep: "Kept whole in the archive",
    prepare: "Prepared once into the archive; bakes read the prepared version",
    store: "Fetched into the store when a bake needs it",
    live: "Read upstream when it is due",
};

export const NEEDS: Record<Need, string> = { held: "Held", download: "Download", prepare: "Prepare", unknown: "Unknown" };

export interface RegionSources {
    region: string;
    name: string;
    boxes: Bbox[];
    countries: string[];
    /** An `incomplete` product: its step list stopped, for `reason`. */
    products: { product: string; incomplete: boolean; reason: string | null }[];
    /** `reason` says why the step list of the layer cannot be listed. */
    layers: { layer: string; need: Need; sources: string[]; reason: string | null }[];
    sources: { source: string; held: Held; for_region: ForRegion }[];
    download_bytes: number;
    /** The sources with a download of unknown size, or whose need is unknown. */
    unknown: string[];
    prepare: string[];
}

export interface ForRegion {
    need: Need;
    requests: number;
    held_requests: number;
    held_bytes: number;
    download_bytes: number;
    unknown_requests: number;
    /** For a source that only a prepare reads: the prepared source whose need it has. */
    read_by: string | null;
    /** The step list of a product that reads it stopped before it read the source. */
    after_stop: boolean;
}

/** `/media/archive · land-polygons`: the archive root and the dataset, a prepared home or `store`,
 *  as `obc data sources` writes it. */
export const whereOf = (held: Held): string => (held.where && held.dataset && held.where !== "store" ? `${held.where} · ${held.dataset}` : (held.where ?? "—"));

/** The bytes held, and what an `archive get` still takes. */
export function sizeOf(held: Held): string {
    const get = held.download_bytes ? [`${size(held.download_bytes)} to get`] : [];
    return [...(held.bytes !== null ? [size(held.bytes)] : []), ...get].join(" · ") || "—";
}

/** What the status leaves out, as `obc data sources` says it: `2 of 5 files in the archive`. */
export function statusNote(held: Held): string[] {
    const files = held.archive_files;
    const notes =
        held.status !== "partial"
            ? []
            : files && files.verified > 0
              ? [`${files.verified} of ${files.listed} files in the archive`]
              : held.store_requests > 0
                ? [count(held.store_requests, "request") + " in the store"]
                : [count(held.store_versions, "version") + " in the store"];
    if (held.gap) notes.push("gap: GLO-30 only");
    if (held.no_dataset) notes.push("no dataset in data/archive/");
    if (held.error) notes.push(held.error);
    return notes;
}

/** The count of each status in `held`, in the order of `STATUSES`. */
export function counts(held: Held[]): [HeldStatus, number][] {
    return (Object.keys(STATUSES) as HeldStatus[]).map((status) => [status, held.filter((at) => at.status === status).length]);
}

/** Whether `row` has `query` in its id, use, coverage or place. */
export function matches(row: SourceRow, query: string): boolean {
    const words = query.trim().toLowerCase();
    return !words || [row.id, row.use, row.group, row.held.covers, row.held.where, row.held.dataset].some((text) => text?.toLowerCase().includes(words));
}

/** What `obc data sources --layer` writes. */
export interface SourceLayers {
    layers: { layer: string; sources: { source: string; held: Held }[] }[];
}

/** What fills the gap of `row`: a step, with the command to type when the page does not run it. A
 *  reference has its prepare beside its versions. */
export function fill(row: SourceRow): { text: string; command?: string[] } | null {
    const held = row.held;
    if (isReference(row) || held.status === "prepared") return null;
    if (held.rule === "prepare") {
        return { text: `The prepare of the source that reads it fetches it.${held.gap ? " Until then the terrain there is GLO-30 only." : ""}` };
    }
    if (held.no_dataset) return { text: "No dataset of data/archive/ lists it. Add one, then get it." };
    if (held.dataset && (held.status === "partial" || held.status === "missing")) {
        return { text: "Get the rest of its dataset into the archive.", command: ["archive", "get", held.dataset] };
    }
    if (held.status === "partial" || held.status === "missing") return { text: "A bake that needs it fetches it into the store." };
    return null;
}

type RegionSource = RegionSources["sources"][number];

/** The layers of `region` that read `source`, or that read the prepared source that reads it. */
export function neededFor(region: RegionSources, source: RegionSource): string[] {
    const by = source.for_region.read_by ?? source.source;
    return region.layers.filter((layer) => layer.sources.includes(by)).map((layer) => layer.layer);
}

/** What a bake of the region holds and lacks of a source, as `obc data sources --region` says it. */
export function needNote({ for_region: need }: RegionSource): string {
    if (need.read_by) return `read by ${need.read_by}`;
    if (need.after_stop) return "after a missing input";
    const notes = [
        ...(need.need === "download" ? [`${need.held_requests} of ${need.requests} fetches held`] : need.requests > 1 ? [`${need.requests} fetches`] : []),
        ...(need.held_bytes ? [`${size(need.held_bytes)}${need.need === "held" ? "" : " held"}`] : []),
    ];
    return notes.join(", ");
}

/** Known bytes and the fetches of unknown size: `1.2 GB and 2 fetches of unknown size`. */
export function bytesNote(bytes: number, unknown: number): string {
    const rest = unknown ? `${unknown} ${unknown === 1 ? "fetch" : "fetches"} of unknown size` : "";
    return bytes ? [size(bytes), rest].filter(Boolean).join(" and ") : rest || "nothing";
}

/** What a bake of `region` downloads once, and what each update reads again: the live OSM
 *  sources. Each is known bytes, and the fetches of unknown size. */
export function totals(region: RegionSources) {
    const sum = (sources: RegionSource[], bytes: (at: ForRegion) => number) => sources.reduce((total, at) => total + bytes(at.for_region), 0);
    const [live, once] = [region.sources.filter((at) => at.held.rule === "live"), region.sources.filter((at) => at.held.rule !== "live")];
    return {
        once: { bytes: sum(once, (at) => at.download_bytes), unknown: sum(once, (at) => at.unknown_requests) },
        osm: { bytes: sum(live, (at) => at.held_bytes + at.download_bytes), unknown: sum(live, (at) => at.unknown_requests) },
        /** The sources whose need is not known, such as those after a missing input. */
        unknownNeed: region.sources.filter((at) => at.for_region.need === "unknown").length,
    };
}

/** How the region map draws a prepared version that the region reads. */
export const VERSION_CELLS = {
    covers: { color: "var(--status-archived)", label: "A version that covers the region" },
    other: { color: "var(--ink-faint)", label: "A version that does not cover it" },
    unknown: { color: "var(--running)", label: "A version of a source of unknown need" },
};

/** The area of each prepared version that `region` reads, that `bounds` finds a box for. A version
 *  covers the region when the source is held and its box holds each box of the region. */
export function versionCells(region: RegionSources, rows: SourceRow[], bounds: (id: string) => Bbox | null) {
    const inside = (box: Bbox) => region.boxes.every((at) => at.west >= box.west && at.east <= box.east && at.south >= box.south && at.north <= box.north);
    return region.sources.flatMap(({ source, for_region }) =>
        (rows.find((row) => row.id === source)?.prepared?.versions ?? []).flatMap((version) => {
            const box = version.region ? bounds(version.region) : null;
            if (!box) return [];
            const kind: keyof typeof VERSION_CELLS = for_region.need === "unknown" ? "unknown" : for_region.need === "held" && inside(box) ? "covers" : "other";
            return [{ box, kind, label: `${source} ${version.version} · ${version.region}` }];
        }),
    );
}
