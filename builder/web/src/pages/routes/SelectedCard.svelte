<!-- The card of the selected route over the map: its picture, its numbers, its profile and its state on the device. -->
<script lang="ts">
    import Credits from '../../components/planner/Credits.svelte';
    import Cover from '../../lib/covers/Cover.svelte';
    import type { Library } from '../../lib/library/store';
    import { ridingProfiles } from '../../lib/planner/riding-profiles';
    import { ROUTES } from '../../lib/routes';
    import { formatDistance, formatHeight } from '../../lib/settings/format';
    import type { Item } from './filter';
    import MiniProfile from './MiniProfile.svelte';
    import { stateText, waitingWith, type DeviceState } from './state';

    let { library, item, device }: { library: Library; item: Item; device?: DeviceState } = $props();

    let credits = $state<string[]>([]), failure = $state('');
    const summary = $derived(item.summary);

    async function send(on: boolean) {
        failure = '';
        try { await library.changeState('waiting', waitingWith(item.id, on)); } catch (reason) { failure = reason instanceof Error ? reason.message : String(reason); }
    }
</script>

<section class="card" aria-label={item.name}>
    <div class="picture">
        <Cover {library} id={item.id} place="card" bind:credits />
        <Credits {credits} side="left" />
    </div>
    <div class="text">
        <h2><a class="open" href={`${ROUTES.route}/${item.id}`}>{item.name}</a></h2>
        {#if summary}
            <p class="meta">{[ridingProfiles[summary.bike].label, (summary.days ?? 1) > 1 && `${summary.days} days`].filter(Boolean).join(' · ')}</p>
            <p class="numbers"><span>{formatDistance(summary.lengthM)}</span><span>{formatHeight(summary.climbM)} up</span></p>
        {/if}
        {#if device}<p class="state">{stateText[device]}</p>{/if}
        {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
    </div>
    {#if summary}<div class="profile"><MiniProfile {summary} /></div>{/if}
    {#if summary}
        <button type="button" class="button" disabled={device === 'sent'} onclick={() => send(device !== 'waiting')}>
            {device === 'waiting' ? 'Do not send' : 'Send to device'}
        </button>
    {/if}
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
        gap: 16px;
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
    /* The name is the one link of the card: it covers the whole card, under the send button and the credits. */
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
    .button {
        position: relative;
        z-index: 1;
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
        .button {
            grid-column: 1 / -1;
            justify-self: start;
        }
    }
</style>
