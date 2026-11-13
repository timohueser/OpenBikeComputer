<!-- The card of the selected route, ride, group of rides or trip over the map: its picture, its numbers, its profile and
     its buttons. The name is the one link of the card. -->
<script lang="ts">
    import Cover from '../../lib/covers/Cover.svelte';
    import type { Library } from '../../lib/library/store';
    import { duration } from '../../lib/planner/editor';
    import { ridingProfiles } from '../../lib/planner/riding-profiles';
    import { ROUTES } from '../../lib/routes';
    import { formatDistance, formatHeight } from '../../lib/settings/format';
    import type { Item } from './filter';
    import MiniProfile from './MiniProfile.svelte';
    import { dayText, tripDates, type Row } from './rows';
    import { stateText, waitingWith, type DeviceState } from './state';

    let { library, item, row, device, route, inGroup = false, credits = $bindable([]) }: {
        library: Library;
        /** A route, or the ride of `row` that the card shows. */
        item: Item;
        /** The row of the ridden view: a ride, a group, whose newest ride `item` is, or a trip, whose first day `item` is. */
        row?: Row;
        device?: DeviceState;
        /** The route of a group, which "Ride it again" opens. */
        route?: string;
        /** The ride is a ride of an open group. */
        inGroup?: boolean;
        /** The credits of the picture. It is too small to show them, so the page shows them with the map. */
        credits?: string[];
    } = $props();

    let failure = $state('');
    const sum = (key: 'lengthM' | 'climbM' | 'movingS') => row!.rides.reduce((total, ride) => total + ride.summary[key], 0);
    const trip = $derived(row?.kind === 'trip');
    // A trip shows the profile and the numbers of all its days.
    const summary = $derived(trip ? { ...item.summary!, line: row!.rides.flatMap(r => r.summary.line), heights: row!.rides.flatMap(r => r.summary.heights),
        lengthM: sum('lengthM'), climbM: sum('climbM'), movingS: sum('movingS') } : item.summary);
    const meta = $derived(row?.kind === 'trip' ? [tripDates(row), `${row.days} days`]
        : row?.kind === 'group' ? [`${row.rides.length} rides`, `last on ${dayText(item.at)}`]
        : item.kind === 'ride' ? [dayText(item.at)]
        : item.summary ? [ridingProfiles[item.summary.bike].label, item.summary.days > 1 && `${item.summary.days} days`] : []);

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

<section class="card" aria-label={row?.name ?? item.name}>
    <div class="picture">
        <Cover {library} id={item.id} place="card" bind:credits />
    </div>
    <div class="text">
        <h2><a class="open" href={`${item.kind === 'ride' ? ROUTES.ride : ROUTES.route}/${item.id}`}>{row?.name ?? item.name}</a></h2>
        {#if summary}
            <p class="meta">{meta.filter(Boolean).join(' · ')}</p>
            <p class="numbers">
                <span>{formatDistance(summary.lengthM)}</span><span>{formatHeight(summary.climbM)} up</span>
                {#if 'movingS' in summary}<span>{duration(summary.movingS / 3600)}</span>{/if}
            </p>
        {/if}
        {#if device}<p class="state">{stateText[device]}</p>{/if}
        {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
    </div>
    {#if summary}<div class="profile"><MiniProfile {summary} /></div>{/if}
    <div class="buttons">
        {#if item.kind === 'route' && summary}
            <button type="button" class="button" disabled={device === 'sent'} onclick={() => send(device !== 'waiting')}>
                {device === 'waiting' ? 'Do not send' : 'Send to device'}
            </button>
        {:else if item.kind === 'ride' && !trip}
            <button type="button" class="button" onclick={again}>Ride it again</button>
            {#if row?.kind === 'group' || inGroup}<button type="button" class="button" onclick={takeOut}>Take out of the group</button>{/if}
        {/if}
    </div>
</section>

<style>
    .card {
        position: relative;
        display: grid;
        grid-template-columns: 220px minmax(0, 1fr) minmax(0, 220px) auto;
        align-items: center;
        gap: 16px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
        box-shadow: 0 6px 20px rgb(28 27 20 / 0.18);
    }
    .picture {
        position: relative;
        height: 130px;
        overflow: hidden;
        border-radius: 6px;
    }
    h2 {
        margin: 0;
        font: 700 18px/1.25 var(--sans);
    }
    p {
        margin: 2px 0 0;
    }
    .meta, .state {
        color: var(--ink-soft);
        font-size: 13px;
    }
    .numbers {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 16px;
        margin-top: 6px;
        font: 700 18px/1.2 var(--sans);
        white-space: nowrap;
    }
    .failure {
        color: var(--coral);
    }
    .profile {
        height: 72px;
    }
    .open {
        color: inherit;
        text-decoration: none;
    }
    /* The name is the one link of the card: it covers the whole card, under the send button. */
    .open::after {
        position: absolute;
        inset: 0;
        border-radius: 10px;
        content: '';
    }
    .open:focus-visible {
        outline: none;
    }
    .open:focus-visible::after {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }
    .buttons {
        position: relative;
        z-index: 1;
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .buttons:empty {
        display: none;
    }
    .button {
        padding: 6px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        white-space: nowrap;
    }
    .button:not(:disabled):hover {
        background: var(--parchment-2);
    }
    @media (max-width: 900px) {
        .card {
            grid-template-columns: 120px minmax(0, 1fr);
        }
        .picture {
            height: 80px;
        }
        .profile {
            display: none;
        }
        .buttons {
            flex-direction: row;
            grid-column: 1 / -1;
            justify-self: start;
        }
    }
</style>
