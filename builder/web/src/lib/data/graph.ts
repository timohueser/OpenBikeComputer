// The layers of a target as a graph. `reads` of a layer names layers of the target and sources.
import type { Layer } from "./shapes";

/** A band for each product, `device/map` in `device`, with a column for each depth of reads: a
 *  layer is one column right of the deepest layer that it reads. */
export function bands(layers: Layer[]): { product: string; columns: Layer[][] }[] {
    const named = new Map(layers.map((layer) => [layer.layer, layer]));
    const depths = new Map<string, number>();
    const depth = (layer: Layer): number => {
        if (!depths.has(layer.layer)) {
            // Before the reads: layers that read each other at layer level end here.
            depths.set(layer.layer, 0);
            const reads = layer.reads.flatMap((name) => named.get(name) ?? []);
            depths.set(layer.layer, Math.max(-1, ...reads.map(depth)) + 1);
        }
        return depths.get(layer.layer)!;
    };
    const bands = new Map<string, Layer[][]>();
    for (const layer of layers) {
        const product = layer.layer.split("/")[0];
        const columns = bands.get(product) ?? [];
        (columns[depth(layer)] ??= []).push(layer);
        bands.set(product, columns);
    }
    return [...bands].map(([product, columns]) => ({ product, columns: columns.filter(Boolean) }));
}

/** The layers that bake again after layer `name`: each layer that reads it, at any depth. */
export function followers(layers: Layer[], name: string): string[] {
    const found = new Set([name]);
    for (let grew = true; grew; ) {
        grew = false;
        for (const layer of layers) {
            if (!found.has(layer.layer) && layer.reads.some((read) => found.has(read))) grew = !!found.add(layer.layer);
        }
    }
    found.delete(name);
    return layers.map((layer) => layer.layer).filter((layer) => found.has(layer));
}

/** The sources that layer `name` reads, at any depth, sorted. */
export function sourcesOf(layers: Layer[], name: string): string[] {
    const named = new Map(layers.map((layer) => [layer.layer, layer]));
    const [sources, seen] = [new Set<string>(), new Set<string>()];
    const visit = (read: string) => {
        const layer = named.get(read);
        if (!layer) sources.add(read);
        else if (!seen.has(read)) {
            seen.add(read);
            layer.reads.forEach(visit);
        }
    };
    named.get(name)?.reads.forEach(visit);
    return [...sources].sort();
}
