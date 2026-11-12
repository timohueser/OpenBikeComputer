<script lang="ts">
    import type { CoverRecord, RideRecord, RouteRecord } from '../library/records';
    import type { Library } from '../library/store';
    import { drawCover, type Crop } from './draw';
    import { lineKey, wantCover } from './job';
    import { nearest } from './line';

    let { library, id, mark, part, line = true, ask = false, onpoint }: {
        library: Library;
        /** The id of a route or the key of a ride. */
        id: string;
        /** Metres along the route, as `along` of line.ts reads them. */
        mark?: number;
        part?: [number, number];
        line?: boolean;
        /** Asks the cover job for a cover when the record has none for its line. */
        ask?: boolean;
        /** The metres along the route of the point nearest to the pointer, or null when the pointer leaves. */
        onpoint?: (metres: number | null) => void;
    } = $props();

    let item = $state<RouteRecord | RideRecord>(), cover = $state<CoverRecord>(), bitmap = $state<ImageBitmap>();
    let canvas = $state<HTMLCanvasElement>(), width = $state(0), height = $state(0), cut: Crop | undefined;
    // The points of a cover of an earlier line draw no mark and no part.
    const summary = $derived(item?.summary && cover?.state === 'done' && cover.lineKey === lineKey(item) ? item.summary : undefined);

    $effect(() => {
        const target = id;
        let live = true;
        const load = async (first: boolean) => {
            const [nextItem, nextCover] = await Promise.all([library.item(target), library.cover(target)]);
            const nextBitmap = nextCover?.image && nextCover.points ? await createImageBitmap(nextCover.image).catch(() => undefined) : undefined;
            if (!live) return nextBitmap?.close();
            [item, cover, bitmap] = [nextItem, nextCover, nextBitmap];
            const done = nextCover?.state !== 'waiting' && nextItem && nextCover?.lineKey === lineKey(nextItem);
            if (first && ask && nextItem?.summary && !done) wantCover(target);
        };
        void load(true);
        const stop = library.onChange((store, key) => { if (key === target && store !== 'state') void load(false); });
        return () => { live = false; stop(); };
    });

    $effect(() => {
        const shown = bitmap;
        return () => shown?.close();
    });

    $effect(() => {
        if (!canvas || !bitmap || !cover?.points || !width || !height) return;
        canvas.width = Math.round(width * devicePixelRatio);
        canvas.height = Math.round(height * devicePixelRatio);
        cut = drawCover(canvas.getContext('2d')!, { image: bitmap, width: bitmap.width, height: bitmap.height, points: cover.points, summary },
            { box: [canvas.width, canvas.height], mark, part, line });
    });

    function point(event: PointerEvent) {
        if (!cut || !summary || !cover?.points || !canvas) return;
        const box = canvas.getBoundingClientRect(), ratio = canvas.width / box.width;
        const metres = nearest(cover.points, summary, (event.clientX - box.left) * ratio / cut.scale + cut.left, (event.clientY - box.top) * ratio / cut.scale + cut.top);
        if (metres !== undefined) onpoint?.(metres);
    }

    /** The line on a plain ground, in a box of 100 by the shape of the line, with the north up. */
    const fallback = $derived.by(() => {
        const flat = item?.summary?.line;
        if (!flat?.length) return undefined;
        const lons = flat.filter((_, i) => i % 2 === 0), lats = flat.filter((_, i) => i % 2 === 1);
        const west = Math.min(...lons), north = Math.max(...lats), across = Math.cos((north + Math.min(...lats)) / 2 * Math.PI / 180);
        const xs = lons.map(lon => (lon - west) * across), ys = lats.map(lat => north - lat);
        const size = Math.max(...xs, ...ys) || 1, pad = 8;
        return {
            box: `${-pad} ${-pad} ${Math.max(...xs) / size * 100 + 2 * pad} ${Math.max(...ys) / size * 100 + 2 * pad}`,
            points: xs.map((x, i) => `${(x / size * 100).toFixed(2)},${(ys[i] / size * 100).toFixed(2)}`).join(' '),
        };
    });
</script>

<div class="cover" bind:clientWidth={width} bind:clientHeight={height}>
    {#if bitmap && cover?.points}
        <canvas bind:this={canvas} onpointermove={point} onpointerleave={() => onpoint?.(null)}></canvas>
    {:else if fallback && line}
        <svg viewBox={fallback.box} preserveAspectRatio="xMidYMid meet" aria-hidden="true">
            <polyline points={fallback.points} />
        </svg>
    {/if}
</div>

<style>
    .cover {
        position: relative;
        width: 100%;
        height: 100%;
        overflow: hidden;
        background: var(--parchment);
    }

    canvas, svg {
        display: block;
        width: 100%;
        height: 100%;
    }

    polyline {
        fill: none;
        stroke: var(--route);
        stroke-width: 3;
        stroke-linejoin: round;
        stroke-linecap: round;
        vector-effect: non-scaling-stroke;
    }
</style>
