<!-- The picture of a signed route: its cover in the live set of route pictures, else its line on one light ground in
     both themes. The line has the colour of the route. -->
<script lang="ts">
    import { holdCover, type RouteCover } from '../../lib/covers/route-pictures';
    import CoverPicture from '../../lib/covers/CoverPicture.svelte';
    import { routeColor, SIGNED_GROUND } from '../../lib/planner/route-colors';
    import type { SignedItem } from '../routes/filter';

    let { item, small = false, oncover }: {
        item: SignedItem;
        /** The small image of the cover, for a list row. */
        small?: boolean;
        /** The credits of the set when the cover shows, and undefined when that cover goes. */
        oncover?: (credits: string[] | undefined) => void;
    } = $props();

    const colour = $derived(routeColor(item.id));
    let cover = $state.raw<RouteCover>();
    // A row shows its picture only near the view: a picture that goes stops its load, and the last one closes the bitmap.
    $effect(() => {
        const { cover: loading, release } = holdCover(item.record, small);
        let live = true, shown = false;
        cover = undefined;
        void loading.then(found => {
            if (!live || !found) return;
            cover = found;
            shown = true;
            oncover?.(found.credits);
        });
        return () => {
            live = false;
            if (shown) oncover?.(undefined);
            release();
        };
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
