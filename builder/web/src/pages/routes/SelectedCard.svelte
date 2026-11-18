<!-- The card of the selected route, ride, group of rides or trip over the map, with the buttons that change the
     library. -->
<script lang="ts">
    import Cover from '../../lib/covers/Cover.svelte';
    import type { Library } from '../../lib/library/store';
    import { duration } from '../../lib/planner/editor';
    import { ridingProfiles } from '../../lib/planner/riding-profiles';
    import { ROUTES } from '../../lib/routes';
    import { formatDistance, formatHeight } from '../../lib/settings/format';
    import type { Item } from './filter';
    import RouteCard from './RouteCard.svelte';
    import { dayText, rowBike, tripDates, type Row } from './rows';
    import { stateText, waitingWith, type DeviceState } from './state';

    let { library, item, row, device, route, runs = 0, inGroup = false, credits = $bindable([]), onclose }: {
        library: Library;
        /** A route, or the ride of `row` that the card shows. */
        item: Item;
        /** The row of the ridden view: a ride, a group, whose newest ride `item` is, or a trip, whose first day `item` is. */
        row?: Row;
        device?: DeviceState;
        /** The route of a group, which "Ride it again" opens. */
        route?: string;
        /** The number of rides on the route. */
        runs?: number;
        /** The ride is a ride of an open group. */
        inGroup?: boolean;
        /** The credits of the picture. It is too small to show them, so the page shows them with the map. */
        credits?: string[];
        onclose: () => void;
    } = $props();

    let failure = $state('');
    const sum = (key: 'lengthM' | 'climbM' | 'movingS') => row!.rides.reduce((total, ride) => total + ride.summary[key], 0);
    const trip = $derived(row?.kind === 'trip');
    // A trip shows the profile and the numbers of all its days.
    const summary = $derived(trip ? { ...item.summary!, line: row!.rides.flatMap(r => r.summary.line), heights: row!.rides.flatMap(r => r.summary.heights),
        lengthM: sum('lengthM'), climbM: sum('climbM'), movingS: sum('movingS') } : item.summary);
    const bike = $derived(row ? rowBike(row) : item.summary?.bike), activity = $derived(bike && ridingProfiles[bike]);
    const meta: (string | false)[] = $derived([activity ? activity.label : false, ...row?.kind === 'trip' ? [tripDates(row), `${row.days} days`]
        : row?.kind === 'group' ? [`${row.rides.length} rides`, `last on ${dayText(item.at)}`]
        : item.kind === 'ride' ? [dayText(item.at)]
        : item.summary ? [item.summary.days > 1 && `${item.summary.days} days`, runs > 0 && (runs === 1 ? 'ridden once' : `ridden ${runs} times`)] : []]);
    const numbers = $derived(summary ? [formatDistance(summary.lengthM), `${formatHeight(summary.climbM)} up`,
        ...'movingS' in summary ? [duration(summary.movingS / 3600)] : []] : []);

    async function act(work: () => Promise<unknown>) {
        failure = '';
        try { await work(); } catch (reason) { failure = reason instanceof Error ? reason.message : String(reason); }
    }
    const send = (on: boolean) => act(() => library.changeState('waiting', waitingWith(item.id, on)));
    const again = () => act(async () => {
        if (route) return void (location.hash = `${ROUTES.route}/${route}`);
        const [{ rideAgain }, ride] = await Promise.all([import('../../lib/ride/again'), library.ride(item.id)]);
        if (ride) await rideAgain(library, ride);
    });
    const takeOut = () => act(() => library.changeState('apart', apart => [...apart ?? [], item.id]));
</script>

<RouteCard name={row?.name ?? item.name} href={`${item.kind === 'ride' ? ROUTES.ride : ROUTES.route}/${item.id}`} icon={activity?.icon} {meta} {numbers}
    profile={summary ?? undefined} state={device && stateText[device]} {failure} {onclose}>
    {#snippet picture()}<Cover {library} id={item.id} place="card" bind:credits />{/snippet}
    {#snippet buttons()}
        {#if item.kind === 'route'}
            {#if item.plan}<a class="button primary" href={`${ROUTES.planner}/${item.id}`}>Open in planner</a>{/if}
            {#if summary}
                <button type="button" class="button quiet" disabled={device === 'sent'} onclick={() => send(device !== 'waiting')}>
                    {device === 'waiting' ? 'Do not send' : 'Send to device'}
                </button>
            {/if}
        {:else if item.kind === 'ride' && !trip}
            <button type="button" class="button" onclick={again}>Ride it again</button>
            {#if row?.kind === 'group' || inGroup}<button type="button" class="button quiet" onclick={takeOut}>Take out of the group</button>{/if}
        {/if}
    {/snippet}
</RouteCard>
