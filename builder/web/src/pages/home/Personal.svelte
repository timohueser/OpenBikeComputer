<!-- The text on the left of the home view: the open plan, the rides of this year, or the slogan. -->
<script lang="ts">
    import { initCore } from '../../lib/core/bridge';
    import type { Library } from '../../lib/library/store';
    import { ROUTES } from '../../lib/routes';
    import { formatDistance, formatHeight } from '../../lib/settings/format';
    import { routeItem } from '../routes/filter';
    import type { HomeText } from './personal';

    let { library, text }: { library: Library; text: HomeText } = $props();

    let adding = $state(false);
    const count = (n: number, one: string) => `${n.toLocaleString('en')} ${one}${n === 1 ? '' : 's'}`;
    const addFiles = () => initCore().then(() => import('../AddFiles.svelte')).then(module => module.default);
</script>

<div class="personal">
    {#if text.kind === 'continue'}
        {@const summary = text.route.summary!}
        <p class="eyebrow">Continue where you stopped</p>
        <h1>{routeItem(text.route).name}</h1>
        <p class="numbers">{[formatDistance(summary.lengthM), `${formatHeight(summary.climbM)} up`, summary.days > 1 && count(summary.days, 'day')].filter(Boolean).join(' · ')}</p>
        <p>You changed it {text.ago}.</p>
        <nav class="buttons" aria-label="Continue">
            <a class="button" href={`${ROUTES.planner}/${text.route.id}`}>Open in the planner</a>
            <a class="button" href={`${ROUTES.planner}/new`}>Plan a new route</a>
        </nav>
    {:else if text.kind === 'year'}
        {@const { totals } = text}
        <p class="eyebrow">This year, {totals.rides && totals.hikes ? 'by bike and on foot' : totals.hikes ? 'on foot' : 'by bike'}</p>
        <h1 class="total">{formatDistance(totals.lengthM)}</h1>
        <p class="numbers">{[totals.rides && count(totals.rides, 'ride'), totals.hikes && count(totals.hikes, 'hike')].filter(Boolean).join(' · ')}</p>
        <p>Your last ride was {text.last.name}, {text.ago}.</p>
        <nav class="buttons" aria-label="This year">
            <a class="button" href={`${ROUTES.ride}/${text.last.key}`}>Look back at it</a>
            <a class="button" href={`${ROUTES.planner}/new`}>Plan a route</a>
        </nav>
    {:else}
        <h1 class="slogan">[Slogan goes here]</h1>
        <p class="lead">Plan a route by bike or on foot. Keep your rides. Take both with you.</p>
        <p class="note">Free. No account is necessary. Your routes stay in this browser.</p>
        <nav class="buttons" aria-label="Start">
            <a class="button primary" href={`${ROUTES.planner}/new`}>Plan a route</a>
            <button type="button" class="button" onclick={() => adding = true}>Add a GPX or FIT file</button>
        </nav>
    {/if}
</div>

{#if adding}
    {#await addFiles() then AddFiles}
        <AddFiles {library} onClose={() => adding = false} />
    {:catch error}
        <p class="failure" role="alert">{error instanceof Error ? error.message : String(error)}</p>
    {/await}
{/if}

<style>
    .personal {
        display: flex;
        flex-direction: column;
        gap: 8px;
        max-width: 520px;
    }
    p {
        margin: 0;
    }
    .eyebrow {
        font: 700 13px/1.3 var(--sans);
        letter-spacing: .06em;
        text-transform: uppercase;
    }
    h1 {
        margin: 0;
        font: 700 44px/1.1 var(--sans);
        overflow-wrap: anywhere;
    }
    .total {
        font-size: 64px;
    }
    .slogan {
        font-size: 48px;
    }
    .numbers, .lead {
        font-size: 18px;
    }
    .note {
        opacity: .85;
    }
    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
        margin-top: 8px;
    }
    .button {
        display: inline-flex;
        align-items: center;
        padding: 6px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: inherit;
        text-decoration: none;
        text-shadow: none;
        white-space: nowrap;
    }
    .button:hover {
        background: var(--parchment-2);
    }
    .primary, .primary:hover {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    .failure {
        color: var(--coral);
    }
    @media (max-width: 760px) {
        h1, .slogan {
            font-size: 34px;
        }
        .total {
            font-size: 48px;
        }
        .numbers, .lead {
            font-size: 16px;
        }
    }
</style>
