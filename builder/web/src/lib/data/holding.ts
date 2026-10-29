// How this machine holds each source: `held` of `obc data sources --json`, and what
// `obc data sources --region ID --json` writes.
import type { Bbox } from "./areas";
import { size } from "./shapes";
import { isReference, PARTS, REFERENCES, type SourceRow } from "./sources";

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

/** A `data` source with `keep = "prepared"` that is not prepared itself, such as `dtm-ch`. */
export const onlyPrepareReads = (row: SourceRow): boolean => row.kind === "data" && row.keep === "prepared" && !isReference(row);

/** `/media/archive · land-polygons`: the archive root and the dataset, a prepared home or `store`. */
export const whereOf = (held: Held): string => [held.where, held.dataset].filter(Boolean).join(" · ") || "—";

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
                ? [`${held.store_requests} ${held.store_requests === 1 ? "request" : "requests"} in the store`]
                : [`${held.store_versions} ${held.store_versions === 1 ? "version" : "versions"} in the store`];
    if (held.gap) notes.push("gap: GLO-30 only");
    if (held.no_dataset) notes.push("no dataset in data/archive/");
    if (held.error) notes.push(held.error);
    return notes;
}

/** The status counts of `rows`, in the order of `STATUSES`. */
export function counts(rows: SourceRow[]): [HeldStatus, number][] {
    return (Object.keys(STATUSES) as HeldStatus[]).map((status) => [status, rows.filter((row) => row.held.status === status).length]);
}

/** Whether `row` has `query` in its id, use, coverage or place. */
export function matches(row: SourceRow, query: string): boolean {
    const words = query.trim().toLowerCase();
    return !words || [row.id, row.use, row.group, row.held.covers, row.held.where, row.held.dataset].some((text) => text?.toLowerCase().includes(words));
}

/** For each name of `used_by`, the `data` sources and assets that it reads, as
 *  `obc data sources --layer` lists them: a source that only a prepare reads is in the prepared one. */
export function layersOf(rows: SourceRow[]): { layer: string; rows: SourceRow[] }[] {
    const read = rows.filter((row) => row.kind !== "tool" && !onlyPrepareReads(row));
    return Object.keys(PARTS).map((layer) => ({ layer, rows: read.filter((row) => row.used_by?.includes(layer)) }));
}

/** The prepared source whose prepare reads `row`: the one that holds it, else the reference of
 *  its national models. */
export const preparedBy = (row: SourceRow): string | null =>
    row.held.prepared_by ?? REFERENCES.find((reference) => reference.models && row.id.startsWith(reference.models.prefix))?.id ?? null;

/** What fills the gap of `row`: a command to type, or the prepare of a source on this page. A
 *  reference has its prepare beside its versions. */
export function fill(row: SourceRow): { text: string; command?: string[]; prepare?: string } | null {
    const held = row.held;
    if (isReference(row)) return null;
    if (onlyPrepareReads(row)) {
        const by = preparedBy(row);
        if (held.status === "prepared") return null;
        const gap = held.gap ? " Until then the terrain there is GLO-30 only." : "";
        return by ? { text: `A prepare of ${by} for its area reads it.${gap}`, prepare: by } : { text: `The prepare that reads it fetches it.${gap}` };
    }
    if (held.no_dataset) return { text: "No dataset of data/archive/ lists it. Add one, then get it." };
    if (held.dataset && (held.status === "partial" || held.status === "missing")) {
        return { text: "Get the rest of its dataset into the archive.", command: ["archive", "get", held.dataset] };
    }
    if (held.status === "partial" || held.status === "missing") return { text: "A bake that needs it fetches it into the store." };
    return null;
}

/** The layers of `region` that read `source`, or that read the prepared source that reads it. */
export function neededFor(region: RegionSources, source: string): string[] {
    const by = region.sources.find((at) => at.source === source)?.for_region.read_by ?? source;
    return region.layers.filter((layer) => layer.sources.includes(by)).map((layer) => layer.layer);
}

/** The bytes of OSM that each update of `region` reads again: the known bytes of its live sources. */
export const osmPerUpdate = (region: RegionSources): number =>
    region.sources.filter((at) => at.held.rule === "live").reduce((sum, at) => sum + at.for_region.held_bytes + at.for_region.download_bytes, 0);
