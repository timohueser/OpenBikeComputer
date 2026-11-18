<!-- The picture of a signed route: its cover in the live set of route pictures, else its line on one light ground in
     both themes. The line has the colour of the route. -->
<script lang="ts">
    import { coverOf, type RouteCover } from '../../lib/covers/route-pictures';
    import CoverPicture from '../../lib/covers/CoverPicture.svelte';
    import { routeColor, SIGNED_GROUND } from '../../lib/planner/route-colors';
    import type { SignedItem } from '../routes/filter';

    let { item, credits = $bindable([]) }: {
        item: SignedItem;
        /** The credits of the shown cover. */
        credits?: string[];
    } = $props();

    const colour = $derived(routeColor(item.id));
    let cover = $state.raw<RouteCover>();
    $effect(() => {
        const record = item.record;
        let live = true;
        cover = undefined;
        credits = [];
        void coverOf(record).then(found => {
            if (!live) return found?.bitmap.close();
            cover = found;
            credits = found?.credits ?? [];
        });
        return () => { live = false; };
    });
    // A row shows its picture only near the view: the bitmap closes when the picture leaves.
    $effect(() => {
        const bitmap = cover?.bitmap;
        return () => bitmap?.close();
    });
</script>

<!-- A row clears the ground of its cover and shows its own, so the wrapper carries the ground as well. -->
<div style:--route={colour} style:--parchment={SIGNED_GROUND} style:background={SIGNED_GROUND}>
    <CoverPicture shown={cover} summary={item.summary} {colour} />
</div>

<style>
    div {
        height: 100%;
    }
</style>
