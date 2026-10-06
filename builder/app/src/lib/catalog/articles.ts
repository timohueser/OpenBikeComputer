// The pinned sparse index of detached article sections on the network grid.
import { formatCellId, parseCellId } from "../core/grid";
import type { Catalog } from "./manifest";
import { arr, fail, int, json, obj, pinnedUrlStr, SHA256, str } from "./parse";

export interface ArtifactPin {
    bytes: number;
    sha256: string;
    url: string;
}

export interface ArticleCellEntry {
    id: string;
    landmarks: ArtifactPin | null;
    peaks: ArtifactPin | null;
}

export interface ArticleIndexDocument {
    cells: ArticleCellEntry[];
    byId: ReadonlyMap<string, ArticleCellEntry>;
}

export function parseArtifactPin(value: unknown, where: string): ArtifactPin {
    const pin = obj(value, where);
    const sha256 = str(pin, "sha256", where, SHA256);
    return { bytes: int(pin, "bytes", where, 1), sha256, url: pinnedUrlStr(pin, "url", sha256, where) };
}

export function parseArticleIndex(body: string, catalog: Catalog): ArticleIndexDocument {
    const where = "article index";
    const doc = obj(json(body, where), where);
    if (doc.schema_version !== catalog.schema_version) fail(`${where}: unsupported schema_version`);
    const network = catalog.schema.bands.find((band) => band.role === "core");
    if (!network) fail(`${where}: the schema has no core band`);
    const seen = new Set<string>();
    const cells = arr(doc.cells, `${where}.cells`).map((value, index): ArticleCellEntry => {
        const at = `${where}.cells[${index}]`;
        const cell = obj(value, at);
        const id = str(cell, "id", at);
        try {
            const parsed = parseCellId(id);
            if (formatCellId(parsed) !== id) fail(`${at}: id is not canonical`);
            if (parsed.log2 !== network.cell_log2) fail(`${at}: id is not on the network grid`);
        } catch (cause) {
            fail(`${at}: ${cause instanceof Error ? cause.message : String(cause)}`);
        }
        if (seen.has(id)) fail(`${at}: duplicate cell id ${id}`);
        seen.add(id);
        const landmarks = cell.landmarks == null ? null : parseArtifactPin(cell.landmarks, `${at}.landmarks`);
        const peaks = cell.peaks == null ? null : parseArtifactPin(cell.peaks, `${at}.peaks`);
        if (!landmarks && !peaks) fail(`${at}: cell has no article artifact`);
        return { id, landmarks, peaks };
    });
    return { cells, byId: new Map(cells.map((cell) => [cell.id, cell])) };
}

export function selectedArticles(
    cellsByBand: ReadonlyMap<string, readonly string[]>,
    catalog: Catalog,
    index: ArticleIndexDocument | null,
): { kind: "landmarks" | "peaks"; id: string; pin: ArtifactPin }[] {
    const network = catalog.schema.bands.find((band) => band.role === "core");
    if (!network || !index) return [];
    return (cellsByBand.get(network.id) ?? []).flatMap((id) => {
        const cell = index.byId.get(id);
        return (["landmarks", "peaks"] as const).flatMap((kind) => {
            const pin = cell?.[kind];
            return pin ? [{ kind, id, pin }] : [];
        });
    });
}
