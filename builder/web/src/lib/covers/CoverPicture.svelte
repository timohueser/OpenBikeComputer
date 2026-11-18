<!-- Draws a cover image with its line, pins and photos, or the line on a plain ground when there is no image. The caller
     owns the bitmap and closes it. -->
<script lang="ts">
    import type { CoverPoints, Summary } from '../library/records';
    import { crop, drawCover, type Crop } from './draw';
    import { lineCoordinates } from '../library/summary';
    import { along, fromAbove, nearest } from './line';
    import Icon from '../ui/Icon.svelte';
    import PhotoMarks from '../../components/route/PhotoMarks.svelte';

    let { shown, summary, breaks = [], look = 'route', colour, mark, part, line = true, pins = [], photos = [], onphoto, onpoint }: {
        /** The image, the points of its line, and the summary that the points belong to. With no summary, the image
         * shows no mark, no part and no pins. */
        shown?: { bitmap: ImageBitmap; points: CoverPoints; summary?: Summary };
        /** The line on a plain ground when there is no image. */
        summary?: Pick<Summary, 'line'>;
        /** The indices of the points of `summary` where a new piece of the line starts, after a gap. */
        breaks?: number[];
        look?: 'route' | 'ride';
        /** The colour of the line on the image, in place of the colours of `look`. */
        colour?: string;
        /** Metres along the route, as `along` of line.ts reads them. */
        mark?: number;
        part?: [number, number];
        line?: boolean;
        /** Labels at shares of the summary line from 0 to 1: a numbered waypoint, or a night without a number. */
        pins?: { share: number; label: string; number?: number }[];
        /** The photos of a ride at shares of the summary line, each with the address of its small picture. */
        photos?: { id: string; url: string; share: number }[];
        onphoto?: (id: string) => void;
        /** The metres along the route of the point nearest to the pointer, or null when the pointer leaves. */
        onpoint?: (metres: number | null) => void;
    } = $props();

    /** The pointer marks the line only within this many CSS pixels of it. */
    const NEAR_PX = 24;
    let canvas = $state<HTMLCanvasElement>(), width = $state(0), height = $state(0), cut: Crop | undefined;

    $effect(() => {
        if (!canvas || !shown || !width || !height) return;
        canvas.width = Math.round(width * devicePixelRatio);
        canvas.height = Math.round(height * devicePixelRatio);
        const { bitmap, points, summary } = shown;
        cut = drawCover(canvas.getContext('2d')!, { image: bitmap, width: bitmap.width, height: bitmap.height, points, summary },
            { box: [canvas.width, canvas.height], mark, part, line, look, colour });
    });

    function point(event: PointerEvent) {
        if (!cut || !shown?.summary || !canvas) return;
        const box = canvas.getBoundingClientRect(), ratio = canvas.width / box.width;
        const metres = nearest(shown.points, shown.summary, (event.clientX - box.left) * ratio / cut.scale + cut.left, (event.clientY - box.top) * ratio / cut.scale + cut.top,
            NEAR_PX * ratio / cut.scale);
        onpoint?.(metres ?? null);
    }

    /** The place of a share of the line in CSS pixels of the box, where the line is visible there. */
    const placeOf = $derived.by(() => {
        const summary = shown?.summary;
        if (!summary || !width || !height) return () => undefined;
        const { bitmap, points } = shown!, ratio = devicePixelRatio;
        const cut = crop({ width: bitmap.width, height: bitmap.height, points }, [Math.round(width * ratio), Math.round(height * ratio)]);
        return (share: number) => {
            const at = along(points, summary, share * summary.lengthM);
            if (!at?.visible) return undefined;
            const x = (at.x - cut.left) * cut.scale / ratio, y = (at.y - cut.top) * cut.scale / ratio;
            return x >= 0 && x <= width && y >= 0 && y <= height ? { x, y } : undefined;
        };
    });
    /** The pins at visible points of the line. `index` is the place in `pins`; `left` puts the label on the side with
     * more room. */
    const shownPins = $derived(pins.flatMap((pin, index) => {
        const at = placeOf(pin.share);
        return at ? [{ ...pin, ...at, index, left: at.x > width / 2 }] : [];
    }));
    const shownPhotos = $derived(photos.flatMap(photo => {
        const at = placeOf(photo.share);
        return at ? [{ ...photo, ...at }] : [];
    }));

    /** The pin whose name shows, by its index; one at a time. */
    let opened = $state<number>();
    const pinName = (pin: { label: string; number?: number }) => pin.number ? pin.label : pin.label ? `Night · ${pin.label}` : 'Night';

    /** The line on a plain ground, in a box of 100 by the shape of the line, with the north up. */
    const fallback = $derived.by(() => {
        if (!summary?.line.length) return undefined;
        const { points, width, height } = fromAbove(lineCoordinates(summary)), pad = 8, ends = [0, ...breaks, points.length];
        const text = points.map(([x, y]) => `${(x * 100).toFixed(2)},${(y * 100).toFixed(2)}`);
        return {
            box: `${-pad} ${-pad} ${width * 100 + 2 * pad} ${height * 100 + 2 * pad}`,
            pieces: ends.slice(1).map((end, i) => text.slice(ends[i], end).join(' ')),
        };
    });
</script>

<svelte:document onpointerdown={event => { if (!(event.target as Element).closest('.route-pin')) opened = undefined; }} />

<div class="cover" bind:clientWidth={width} bind:clientHeight={height}>
    {#if shown}
        <canvas bind:this={canvas} onpointermove={point} onpointerleave={() => onpoint?.(null)}></canvas>
        {#each shownPins as pin (pin.index)}
            <span class="route-pin" style:left={`${pin.x}px`} style:top={`${pin.y}px`}>
                <button type="button" class:night={!pin.number} aria-label={pinName(pin)} aria-expanded={opened === pin.index}
                    onclick={() => opened = pin.index} onfocus={() => opened = pin.index} onblur={() => opened = undefined}
                    onkeydown={event => { if (event.key === 'Escape') opened = undefined; }}>
                    {#if pin.number}{pin.number}{:else}<Icon name="moon" size={12} />{/if}
                </button>
                {#if opened === pin.index}<span class="route-pin-label" class:left={pin.left}>{pinName(pin)}</span>{/if}
            </span>
        {/each}
        <PhotoMarks photos={shownPhotos} onopen={onphoto} />
    {:else if fallback && line}
        <svg class={look} viewBox={fallback.box} preserveAspectRatio="xMidYMid meet" aria-hidden="true">
            {#each fallback.pieces as points, i (i)}<polyline {points} />{/each}
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

    .ride polyline {
        stroke: var(--ride);
    }
</style>
