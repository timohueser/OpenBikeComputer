<script lang="ts">
    import Credits from '../planner/Credits.svelte';
    import RouteMap from './RouteMap.svelte';
    import Cover from '../../lib/covers/Cover.svelte';
    import { settled, wantCover } from '../../lib/covers/job';
    import type { CoverRecord, Summary } from '../../lib/library/records';
    import type { Library } from '../../lib/library/store';
    import { creditsWith } from '../../lib/planner/map-setup';

    /** The large picture of a detail page: the cover large and the map small in a corner, or the map alone with no cover.
     * A click on the small one swaps them. The pointer is a share of the summary line from 0 to 1, so the cover and the
     * map mark the same place. */
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

<div class="picture" class:map-large={mapLarge}>
    {#if covered}
        <div class="slot cover">
            <Cover {library} {id} {pins} mark={mark === undefined ? undefined : mark * summary.lengthM}
                onpoint={metres => onpoint(metres === null || !summary.lengthM ? null : metres / summary.lengthM)} />
            {#if mapLarge}<button type="button" class="swap" aria-label="Show the picture large" onclick={() => swapped = false}></button>{/if}
        </div>
        <Credits {credits} side="left" />
    {/if}
    <div class="slot map">
        <RouteMap {summary} {ride} {pins} {mark} {onpoint} />
        {#if !mapLarge}<button type="button" class="swap" aria-label="Show the map large" onclick={() => swapped = true}></button>{/if}
    </div>
</div>

<style>
    .picture {
        position: relative;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--parchment);
    }
    /* The large place fills the picture; the small place is a corner of it. */
    .slot {
        position: absolute;
        inset: 0;
    }
    .picture:not(.map-large) .map, .map-large .cover {
        inset: auto 12px 12px auto;
        z-index: 2;
        width: 180px;
        height: 110px;
        overflow: hidden;
        border: 3px solid var(--panel);
        border-radius: 8px;
        box-shadow: 0 4px 14px rgb(28 27 20 / 0.25);
    }
    /* The attribution shows with the map in the large place. */
    .picture:not(.map-large) .map :global(.maplibregl-ctrl-attrib) {
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
