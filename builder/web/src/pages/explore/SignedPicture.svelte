<!-- The plain picture of a signed route, in the colour of the route, on a light ground also in the dark theme. -->
<script lang="ts">
    import CoverPicture from '../../lib/covers/CoverPicture.svelte';
    import { routeColor, SIGNED_GROUND } from '../../lib/planner/route-colors';
    import { appearance } from '../../lib/theme.svelte';
    import type { SignedItem } from '../routes/filter';

    let { item }: { item: SignedItem } = $props();
    // A row clears the ground of its cover and shows its own, so the wrapper carries the light ground as well.
    const ground = $derived(appearance.dark ? SIGNED_GROUND : undefined);
</script>

<div style:--route={routeColor(item.id)} style:--parchment={ground} style:background={ground}>
    <CoverPicture summary={item.summary} />
</div>

<style>
    div {
        height: 100%;
    }
</style>
