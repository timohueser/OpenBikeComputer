<!-- A route or a ride in the home drawer: its picture, its name as the link of the card, and a short line. The picture is too
     small for its credits, so the card gives them to the page. -->
<script lang="ts">
    import Cover from '../../lib/covers/Cover.svelte';
    import type { Library } from '../../lib/library/store';
    import { ROUTES } from '../../lib/routes';
    import { formatDistance } from '../../lib/settings/format';
    import type { Item } from '../routes/filter';

    let { library, item, oncredits }: { library: Library; item: Item; oncredits: (credits: string[]) => void } = $props();

    let credits = $state<string[]>([]);
    $effect(() => oncredits(credits));
    const meta = $derived(item.kind === 'ride'
        ? ['Ride', new Date(item.at).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' }), formatDistance(item.summary.lengthM)]
        : ['Route', item.summary && item.summary.days > 1 && `${item.summary.days} days`, item.summary && formatDistance(item.summary.lengthM)]);
</script>

<li class="card">
    <div class="picture">
        {#if item.summary}<Cover {library} id={item.id} place="card" bind:credits />{/if}
    </div>
    <a class="open" href={`${item.kind === 'ride' ? ROUTES.ride : ROUTES.route}/${item.id}`}>{item.name}</a>
    <p class="meta">{meta.filter(Boolean).join(' · ')}</p>
</li>

<style>
    .card {
        position: relative;
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 0;
        padding: 0 10px 10px;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--parchment);
    }
    .card:hover {
        border-color: var(--line-strong);
    }
    .picture {
        position: relative;
        aspect-ratio: 3;
        min-height: 92px;
        max-height: 112px;
        margin: 0 -10px 6px;
        overflow: hidden;
    }
    /* With no picture, the line has room around it on the ground of a picture. */
    .picture :global(.cover) {
        background: var(--parchment-2);
    }
    .picture :global(svg) {
        box-sizing: border-box;
        padding: 12px;
    }
    .open {
        overflow: hidden;
        color: var(--ink);
        font-weight: 700;
        text-decoration: none;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    /* The name is the one link of the card: it covers the whole card. */
    .open::after {
        position: absolute;
        inset: 0;
        content: '';
    }
    .open:focus-visible {
        outline: none;
    }
    .open:focus-visible::after {
        outline: 2px solid var(--forest);
        outline-offset: -2px;
    }
    .meta {
        margin: 0;
        color: var(--ink-soft);
        font-size: 12px;
    }
</style>
