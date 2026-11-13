<!-- The home view behind the hero, and the box that names it. With no live set, nothing: the hero keeps its plain ground.
     The data credits of the view and of the pictures of the cards are in the box, else in a fold-out alone. -->
<script lang="ts">
    import { onMount } from 'svelte';
    import { router } from '../../lib/router.svelte';
    import { ROUTES } from '../../lib/routes';
    import { imageUrl, loadManifest, viewAt, type Manifest } from './views';

    let { now, cards = [], shown = $bindable(false) }: {
        now: Date;
        /** The credits of the pictures of the cards, which show no credits of their own. */
        cards?: string[];
        /** A view shows, so the text over it is light. */
        shown?: boolean;
    } = $props();

    let manifest = $state.raw<Manifest>(), broken = $state(false);
    let retried = false;
    const chosen = $derived(manifest && !broken ? viewAt(manifest, now) : undefined);
    const credits = $derived([...new Set([...chosen ? manifest!.credits : [], ...cards])]);

    $effect(() => { shown = !!chosen; });
    onMount(() => { void loadManifest().then(next => { manifest = next; }); });

    /** An image that fails means that its set is no longer live: a new set shows, the same set or none gives the plain ground. */
    async function failed() {
        const before = manifest?.set;
        if (retried) return void (broken = true);
        retried = true;
        const next = await loadManifest(true);
        if (next && next.set !== before) manifest = next;
        else broken = true;
    }
</script>

{#snippet dataCredits()}
    <details class="credits">
        <summary>Data credits</summary>
        <p>{credits.join(' · ')}</p>
    </details>
{/snippet}

{#if chosen}
    <img class="backdrop" src={imageUrl(manifest!, chosen.view, chosen.slot)} alt="" decoding="async" onerror={failed} />
    <div class="shade"></div>
    <aside class="box" aria-label="You look at">
        <h2>{chosen.view.name}</h2>
        <p class="region">{chosen.view.region}</p>
        <button type="button" class="plan" onclick={() => router.go(`${ROUTES.planner}/new`, { place: { lon: chosen.view.lon, lat: chosen.view.lat } })}>
            Plan a route here
        </button>
        {#if credits.length}{@render dataCredits()}{/if}
    </aside>
{:else}
    {#if credits.length}<div class="box alone">{@render dataCredits()}</div>{/if}
{/if}

<style>
    .backdrop {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
        object-fit: cover;
    }
    .box {
        position: absolute;
        z-index: 1;
        right: 40px;
        bottom: 32px;
        display: grid;
        grid-template-columns: auto 1fr;
        min-width: 260px;
        max-width: 340px;
        padding: 12px 16px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
        color: var(--ink);
        box-shadow: 0 2px 10px rgb(28 27 20 / 0.16);
        text-shadow: none;
    }
    p {
        margin: 0;
    }
    h2, .region {
        grid-column: 1 / -1;
    }
    .region {
        color: var(--ink-soft);
        font-size: 13px;
    }
    h2 {
        margin: 0 0 1px;
        font: 700 16px/1.3 var(--sans);
    }
    /* A soft shade under the text, so that light text is readable on a bright view. */
    .shade {
        position: absolute;
        inset: 0;
        background: linear-gradient(100deg, rgb(0 0 0 / 0.5), rgb(0 0 0 / 0.24) 36%, transparent 62%);
    }
    .alone {
        display: block;
        min-width: 0;
        padding: 4px 10px;
    }
    .alone .credits {
        margin: 0;
    }
    /* The one link of the box and the fold-out of the credits share a line; the open credits take the width under it. */
    .plan, .credits {
        grid-row: 3;
        margin-top: 10px;
        line-height: 20px;
    }
    .plan {
        z-index: 1;
        grid-column: 1;
        align-self: start;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink);
        text-decoration: underline;
        text-underline-offset: 3px;
        white-space: nowrap;
    }
    .credits {
        grid-column: 1 / -1;
        color: var(--ink-soft);
        font-size: 12px;
    }
    .credits summary {
        text-align: right;
        cursor: pointer;
    }
    .alone summary {
        text-align: left;
    }
    .credits p {
        max-height: 120px;
        margin-top: 4px;
        overflow: auto;
    }
    /* On a phone the box has the width of the hero, at its bottom, in the place that the hero keeps free. */
    @media (max-width: 760px) {
        .shade {
            background: linear-gradient(180deg, rgb(0 0 0 / 0.5), rgb(0 0 0 / 0.24) 40%, transparent 65%);
        }
        .box {
            right: 16px;
            bottom: 16px;
            left: 16px;
            max-width: none;
        }
        .alone {
            left: auto;
        }
    }
</style>
