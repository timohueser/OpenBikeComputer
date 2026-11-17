<!-- The plain picture of a signed route, in the colour of the route, on one light ground in both themes. -->
<script lang="ts">
    import CoverPicture from '../../lib/covers/CoverPicture.svelte';
    import { routeColor, SIGNED_GROUND } from '../../lib/planner/route-colors';
    import type { SignedItem } from '../routes/filter';

    let { item }: { item: SignedItem } = $props();
</script>

<!-- A row clears the ground of its cover and shows its own, so the wrapper carries the ground as well. -->
<div style:--route={routeColor(item.id)} style:--parchment={SIGNED_GROUND} style:background={SIGNED_GROUND}>
    <CoverPicture summary={item.summary} />
</div>

<style>
    div {
        height: 100%;
    }
</style>
