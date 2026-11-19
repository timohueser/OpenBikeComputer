<script lang="ts" module>
    import { formatAlong, formatDistance } from '../../lib/settings/format';
    import type { RoutePlace } from '../../lib/planner/place-index';

    const AT_ROUTE_M = 50;

    /** Where along the route a place is, and how far from it: "km 42 · 300 m from the route". A few metres say nothing
     * to a rider, so under AT_ROUTE_M it is "at the route". */
    export function placeAside({ alongM, offM }: RoutePlace): string {
        return `${formatAlong(alongM)} · ${offM < AT_ROUTE_M ? 'at the route' : `${formatDistance(offM)} from the route`}`;
    }
</script>

<script lang="ts">
    import Callout from '../planner/Callout.svelte';
    import PlaceDetails from '../planner/PlaceDetails.svelte';
    import { placeSource } from '../../lib/planner/place-index';
    import { placeDetails, withDetails, type SearchPlace } from '../../lib/planner/search/types';

    /** The popup of a place on the map of the Route page: its details, which load when it opens, and no plan actions. */
    let { routePlace, onClose }: { routePlace: RoutePlace; onClose: () => void } = $props();

    let found = $state.raw<SearchPlace>(), error = $state('');

    $effect(() => {
        const { place } = routePlace, source = placeSource(place.id);
        if (!source) return;
        const abort = new AbortController();
        placeDetails(source, place.coordinate, abort.signal).then(
            answer => { found = answer; },
            () => { if (!abort.signal.aborted) error = 'Place details are unavailable. Try selecting the place again.'; });
        return () => abort.abort();
    });
</script>

<Callout label={routePlace.place.label} {onClose}>
    <PlaceDetails place={withDetails(routePlace.place, found)} aside={placeAside(routePlace)} {error} />
</Callout>
