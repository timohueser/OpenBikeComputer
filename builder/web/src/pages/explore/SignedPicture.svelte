<!-- The plain picture of a signed route or a trail, in the colour of the route, on one light ground in both themes. -->
<script lang="ts">
    import CoverPicture from '../../lib/covers/CoverPicture.svelte';
    import { routeColor, SIGNED_GROUND } from '../../lib/planner/route-colors';
    import type { SignedItem, TrailItem } from '../routes/filter';

    let { item }: { item: SignedItem | TrailItem } = $props();

    // The far line of a trail breaks between its pieces.
    const breaks = $derived.by(() => {
        if (item.kind !== 'trail') return undefined;
        let at = 0;
        return item.entry.far_udeg.slice(0, -1).map(piece => at += piece.length / 2);
    });
</script>

<!-- A row clears the ground of its cover and shows its own, so the wrapper carries the ground as well. -->
<div style:--route={routeColor(item.id)} style:--parchment={SIGNED_GROUND} style:background={SIGNED_GROUND}>
    <CoverPicture summary={item.summary} {breaks} />
</div>

<style>
    div {
        height: 100%;
    }
</style>
