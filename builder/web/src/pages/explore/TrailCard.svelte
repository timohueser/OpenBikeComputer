<!-- The card of the selected trail over the map: its far line, its mark and name, its shape, its stages and its kind, its
     numbers, the profile of the whole trail, the strip of its parts and the credit of its source. -->
<script module lang="ts">
    import type { TrailEntry } from '../../lib/planner/signed-routes';
    import { signedNumbers } from './SignedCard.svelte';
    import { stageCount } from './strip';

    /** The second line of a row of the far list. A far file does not say which stages the catalog holds. */
    export const trailParts = (entry: TrailEntry) => [...signedNumbers(entry), entry.loop && 'Loop', stageCount(entry.stage_count)];
</script>

<script lang="ts">
    import TrailMarker from '../../components/planner/TrailMarker.svelte';
    import { routeColor } from '../../lib/planner/route-colors';
    import type { CatalogRecord } from '../../lib/planner/signed-routes';
    import type { TrailItem } from '../routes/filter';
    import RouteCard from '../routes/RouteCard.svelte';
    import { kindName } from './SignedCard.svelte';
    import SignedPicture from './SignedPicture.svelte';
    import StageStrip from './StageStrip.svelte';

    let { about: trail, item, record, failed, onclose }: {
        /** The record or the far entry of the trail. */
        about: CatalogRecord | TrailEntry;
        /** The trail of the far files, for the picture. */
        item?: TrailItem;
        /** The record of the trail: first that of a cell file if there is one, then the whole record. */
        record?: CatalogRecord;
        failed: boolean;
        onclose: () => void;
    } = $props();

    // The entries of `trail_profile_m` are at equal shares of the trail: on a line of equal steps, MiniProfile places them so.
    const profile = $derived(record?.trail_profile_m && { line: record.trail_profile_m.flatMap((_, i) => [i / 1000, 0]), heights: record.trail_profile_m });
</script>

<RouteCard name={trail.name ?? trail.ref ?? ''} meta={[trail.loop ? 'Loop' : 'One way', stageCount(trail.stage_count, record?.parts), kindName(trail)]} numbers={signedNumbers(trail)}
    {profile} failure={failed ? 'The trail could not load.' : undefined} more={record?.parts || record?.credit ? more : undefined} {onclose}>
    {#snippet picture()}{#if item}<SignedPicture {item} />{/if}{/snippet}
    {#snippet mark()}<span class="mark"><TrailMarker symbol={trail.symbol} ref={trail.ref} /></span>{/snippet}
</RouteCard>

{#snippet more()}
    {#if record?.parts}<StageStrip parts={record.parts} numbered={(trail.stage_count ?? 0) >= 2} color={routeColor(trail.id)} />{/if}
    {#if record?.credit}<p class="credit">Route data: {record.credit}</p>{/if}
{/snippet}

<style>
    /* The mark stands on the first line of the name. */
    .mark {
        display: inline-block;
        margin-right: 8px;
        vertical-align: -5px;
    }
    .credit {
        margin: 8px 0 0;
        color: var(--ink-soft);
        font-size: 13px;
    }
</style>
