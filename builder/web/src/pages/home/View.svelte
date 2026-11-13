<!-- The home view behind the hero, and the box that names it. With no live set, drawn hills in the colours of the theme
     and no box. The data credits of the view and of the pictures of the cards are in the box, else in a fold-out alone. -->
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

    /** An image that fails means that its set is no longer live: a new set shows, the same set or none gives the hills. */
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
    <svg class="backdrop hills" viewBox="0 0 1600 900" preserveAspectRatio="xMidYMax slice" aria-hidden="true">
        <defs>
            <linearGradient id="home-sky" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" style="stop-color: var(--sky-top)" />
                <stop offset="1" style="stop-color: var(--sky-low)" />
            </linearGradient>
        </defs>
        <rect width="1600" height="900" fill="url(#home-sky)" />
        <path class="far" d="M0 520 L140 470 L280 500 L430 440 L580 485 L720 445 L870 475 L1010 430 L1160 465 L1310 420 L1460 455 L1600 425 V900 H0Z" />
        <path class="peak" d="M820 600 L1000 340 L1035 370 L1085 290 L1250 600Z" />
        <path class="face" d="M1085 290 L1108 392 L1086 470 L1128 600 H1250Z" />
        <path class="snow" d="M1085 290 L1120 355 L1102 346 L1083 362 L1066 346 L1048 352Z" />
        <path class="mid" d="M0 640 C200 580 360 600 520 620 S820 560 1000 600 S1300 640 1600 580 V900 H0Z" />
        <path class="near" d="M0 720 C240 660 420 700 640 690 S1000 640 1240 700 S1480 720 1600 690 V900 H0Z" />
        <path class="front" d="M0 800 C300 760 520 800 800 780 S1260 750 1600 790 V900 H0Z" />
    </svg>
{/if}

<style>
    .backdrop {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
        object-fit: cover;
    }
    .hills {
        --sky-top: #c6d4dc;
        --sky-low: #eef0e8;
        --far: #b5c2c8;
        --peak: #8fa3b1;
        --face: #7d93a2;
        --snow: #f5f7f5;
        --mid: #93ab9f;
        --near: #6f9483;
        --front: #4f7462;
    }
    :global(:root[data-theme="dark"]) .hills {
        --sky-top: #0e1522;
        --sky-low: #1f2634;
        --far: #2a313f;
        --peak: #353e4e;
        --face: #2c3443;
        --snow: #8a94a1;
        --mid: #252f2b;
        --near: #1e2924;
        --front: #172019;
    }
    .far { fill: var(--far); }
    .peak { fill: var(--peak); }
    .face { fill: var(--face); }
    .snow { fill: var(--snow); }
    .mid { fill: var(--mid); }
    .near { fill: var(--near); }
    .front { fill: var(--front); }
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
    /* On a phone the box follows the text, at the bottom of the hero. */
    @media (max-width: 760px) {
        .shade {
            background: linear-gradient(180deg, rgb(0 0 0 / 0.5), rgb(0 0 0 / 0.24) 40%, transparent 65%);
        }
        /* The peak stays in the narrow hero. */
        .hills {
            left: -67%;
            width: 167%;
        }
        .box {
            position: relative;
            right: auto;
            bottom: auto;
            order: 2;
            max-width: none;
            margin-top: auto;
        }
    }
</style>
