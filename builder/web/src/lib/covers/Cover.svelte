<script lang="ts">
    import type { CoverPoints, CoverRecord, RideRecord, RouteRecord, Summary } from '../library/records';
    import type { Library } from '../library/store';
    import { appearance } from '../theme.svelte';
    import { crop, drawCover, type Crop } from './draw';
    import { lineKey } from '../library/summary';
    import { settled, wantCover, wantsPicture, type Place } from './job';
    import { along, nearest } from './line';

    let { library, id, mark, part, line = true, place = 'row', pins = [], onpoint }: {
        library: Library;
        /** The id of a route or the key of a ride. */
        id: string;
        /** Metres along the route, as `along` of line.ts reads them. */
        mark?: number;
        part?: [number, number];
        line?: boolean;
        /** A card shows the map picture when the record has no cover. A card and a page ask the cover job for what is missing. */
        place?: Place;
        /** Labels at shares of the summary line from 0 to 1: a numbered waypoint, or a night without a number. */
        pins?: { share: number; label: string; number?: number }[];
        /** The metres along the route of the point nearest to the pointer, or null when the pointer leaves. */
        onpoint?: (metres: number | null) => void;
    } = $props();

    /** The pointer marks the line only within this many CSS pixels of it. */
    const NEAR_PX = 24;
    /** The image that the part shows, with the points of its line, and the summary that the points belong to. */
    interface Shown { bitmap: ImageBitmap; points: CoverPoints; summary?: Summary }
    let item = $state<RouteRecord | RideRecord>(), shown = $state<Shown>();
    let canvas = $state<HTMLCanvasElement>(), width = $state(0), height = $state(0), cut: Crop | undefined;

    /** The cover, else on a card the map picture of the theme for the line. */
    function source(item: RouteRecord | RideRecord, cover: CoverRecord, dark: boolean) {
        const current = cover.lineKey === lineKey(item), summary = item.summary ?? undefined;
        // The points of a cover of an earlier line draw no mark and no part.
        if (cover.image && cover.points) return { image: cover.image, points: cover.points, summary: current && cover.state === 'done' ? summary : undefined };
        const map = place === 'card' && cover.map?.lineKey === lineKey(item) ? cover.map : undefined, image = map?.[dark ? 'dark' : 'light'];
        return image && map?.points ? { image, points: map.points, summary } : undefined;
    }

    // A card loads again after a change of the theme, and asks for the picture of the new theme.
    $effect(() => {
        const target = id, dark = place === 'card' && appearance.dark;
        let live = true;
        const load = async (first: boolean) => {
            const [nextItem, nextCover] = await Promise.all([library.item(target), library.cover(target)]);
            const from = nextItem && nextCover && source(nextItem, nextCover, dark);
            const bitmap = from ? await createImageBitmap(from.image).catch(() => undefined) : undefined;
            if (!live) return bitmap?.close();
            [item, shown] = [nextItem, from && bitmap && { bitmap, points: from.points, summary: from.summary }];
            if (!first || place === 'row' || !nextItem?.summary) return;
            if (!settled(nextItem, nextCover) || (place === 'card' && wantsPicture(nextItem, nextCover, dark))) wantCover(target, place);
        };
        void load(true);
        const stop = library.onChange((store, key) => { if (key === target && store !== 'state') void load(false); });
        return () => { live = false; stop(); };
    });

    $effect(() => {
        const bitmap = shown?.bitmap;
        return () => bitmap?.close();
    });

    $effect(() => {
        if (!canvas || !shown || !width || !height) return;
        canvas.width = Math.round(width * devicePixelRatio);
        canvas.height = Math.round(height * devicePixelRatio);
        const { bitmap, points, summary } = shown;
        cut = drawCover(canvas.getContext('2d')!, { image: bitmap, width: bitmap.width, height: bitmap.height, points, summary },
            { box: [canvas.width, canvas.height], mark, part, line });
    });

    function point(event: PointerEvent) {
        if (!cut || !shown?.summary || !canvas) return;
        const box = canvas.getBoundingClientRect(), ratio = canvas.width / box.width;
        const metres = nearest(shown.points, shown.summary, (event.clientX - box.left) * ratio / cut.scale + cut.left, (event.clientY - box.top) * ratio / cut.scale + cut.top,
            NEAR_PX * ratio / cut.scale);
        onpoint?.(metres ?? null);
    }

    /** The pins at visible points of the line, in CSS pixels of the box. */
    const shownPins = $derived.by(() => {
        const summary = shown?.summary;
        if (!summary || !width || !height) return [];
        const { bitmap, points } = shown!, ratio = devicePixelRatio;
        const cut = crop({ width: bitmap.width, height: bitmap.height, points }, [Math.round(width * ratio), Math.round(height * ratio)]);
        return pins.flatMap(pin => {
            const at = along(points, summary, pin.share * summary.lengthM);
            const x = at && (at.x - cut.left) * cut.scale / ratio, y = at && (at.y - cut.top) * cut.scale / ratio;
            return at?.visible && x! >= 0 && x! <= width && y! >= 0 && y! <= height ? [{ ...pin, x: x!, y: y! }] : [];
        });
    });

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
    {#if shown}
        <canvas bind:this={canvas} onpointermove={point} onpointerleave={() => onpoint?.(null)}></canvas>
        {#each shownPins as pin (pin.share)}
            <span class="route-pin" style:left={`${pin.x}px`} style:top={`${pin.y}px`}>
                {#if pin.number}<b>{pin.number}</b>{pin.label}{:else}Night{pin.label ? ` · ${pin.label}` : ''}{/if}
            </span>
        {/each}
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

    .route-pin {
        position: absolute;
        transform: translate(-50%, calc(-100% - 10px));
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
