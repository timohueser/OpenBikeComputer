<script lang="ts">
    import Credits from '../planner/Credits.svelte';
    import RouteMap from './RouteMap.svelte';
    import Cover from '../../lib/covers/Cover.svelte';
    import { HEIGHT, settled, wantCover, WIDTH } from '../../lib/covers/job';
    import type { CoverRecord, Summary } from '../../lib/library/records';
    import type { Library } from '../../lib/library/store';
    import { creditsWith } from '../../lib/planner/map-setup';

    /** The large picture of a detail page: the whole cover at the full height with the map beside it, or the map alone
     * with no cover. When the map has too little room beside the cover, the cover is large and cut, and the map is small in
     * a corner; a click on the small one swaps them, and only the large one shows the pins. The pointer is a share of the
     * summary line from 0 to 1, so the cover and the map mark the same place. */
    let { library, id, summary, ride = false, pins = [], mark, onpoint }: {
        library: Library;
        /** The id of a route or the key of a ride. */
        id: string;
        summary: Summary;
        ride?: boolean;
        pins?: { share: number; label: string; number?: number }[];
        mark?: number;
        onpoint: (share: number | null) => void;
    } = $props();

    let cover = $state.raw<CoverRecord>(), swapped = $state(false);
    const covered = $derived(!!cover?.image);
    // The small map draws the basemap too. A cover made before its credits were kept has the credits of the release.
    const credits = $derived(creditsWith(cover?.credits ?? []));
    // A cover takes the large place, unless the rider swapped by hand.
    const mapLarge = $derived(!covered || swapped);
    /** The least width of the map beside the cover, and the gap between the cards of the page. */
    const MAP_PX = 260, GAP_PX = 14;
    let width = $state(0), height = $state(0);
    // The cards have a border of 1 px. A CSS container query cannot compare the width with a share of the height, so
    // the part measures its box.
    const coverWidth = $derived(Math.round((height - 2) * WIDTH / HEIGHT) + 2);
    const side = $derived(covered && width - coverWidth - GAP_PX >= MAP_PX);

    $effect(() => {
        const target = id;
        let live = true;
        [cover, swapped] = [undefined, false];
        const load = async (first: boolean) => {
            const [item, stored] = await Promise.all([library.item(target), library.cover(target)]);
            if (!live) return;
            cover = stored;
            // Also the cover of an earlier line, which shows until the new one arrives.
            if (first && item?.summary && !settled(item, stored)) wantCover(target, 'page');
        };
        void load(true);
        const stop = library.onChange((store, key) => { if (key === target && store !== 'state') void load(false); });
        return () => { live = false; stop(); };
    });
</script>

<div class="picture" class:side class:map-large={!side && mapLarge} bind:clientWidth={width} bind:clientHeight={height}
    style:--cover-width={`${coverWidth}px`} style:--map-left={`${coverWidth + GAP_PX}px`}>
    {#if covered}
        <div class="slot cover">
            <Cover {library} {id} pins={side || !mapLarge ? pins : []} mark={mark === undefined ? undefined : mark * summary.lengthM}
                onpoint={metres => onpoint(metres === null || !summary.lengthM ? null : metres / summary.lengthM)} />
            {#if !side && mapLarge}<button type="button" class="swap" aria-label="Show the picture large" onclick={() => swapped = false}></button>{/if}
        </div>
        <Credits {credits} side="left" />
    {/if}
    <div class="slot map">
        <RouteMap {summary} {ride} pins={side || mapLarge ? pins : []} {mark} {onpoint} />
        {#if !side && !mapLarge}<button type="button" class="swap" aria-label="Show the map large" onclick={() => swapped = true}></button>{/if}
    </div>
</div>

<style>
    .picture {
        position: relative;
    }
    /* A card fills the picture, or with `side` the cover is on the left and the map beside it; the small place is a
     * corner of the large card. */
    .slot {
        position: absolute;
        inset: 0;
        box-sizing: border-box;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--parchment);
    }
    .side .cover {
        width: var(--cover-width);
    }
    .side .map {
        left: var(--map-left);
    }
    .picture:not(.map-large) .map, .map-large .cover {
        inset: auto 12px 12px auto;
        z-index: 2;
        width: 180px;
        height: 110px;
        border: 3px solid var(--panel);
        border-radius: 8px;
        box-shadow: 0 4px 14px rgb(28 27 20 / 0.25);
    }
    /* The attribution shows with the map in the large place. */
    .picture:not(.map-large, .side) .map :global(.maplibregl-ctrl-attrib) {
        display: none;
    }
    .swap {
        position: absolute;
        z-index: 3;
        inset: 0;
        padding: 0;
        border: 0;
        background: none;
    }
    @media (max-width: 760px) {
        .picture {
            order: -1;
            aspect-ratio: 3 / 2;
        }
        .picture:not(.map-large) .map, .map-large .cover {
            width: 120px;
            height: 80px;
        }
    }
</style>
