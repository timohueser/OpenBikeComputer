<script lang="ts">
    import type { EngineRoute } from '../../lib/planner/routing';
    let { routes, choiceId, status = '', onPick }: { routes: EngineRoute[]; status?: string; choiceId: string; onPick: (route: EngineRoute) => void } = $props();
    function title(id: string) { return id.endsWith('/shorter') ? 'Shorter' : id.endsWith('/smoother') ? 'Smoother' : id.endsWith('/less-climbing') ? 'Less climbing' : 'Balanced'; }
    function differences(route: EngineRoute) {
        const a = routes[0].totals, b = route.totals;
        const signed = (n: number, digits = 0) => `${n > 0 ? '+' : ''}${n.toFixed(digits)}`;
        const unpaved = (r: EngineRoute) => r.totals.surface_m.slice(3).reduce((sum, n) => sum + n, 0);
        return `${signed((b.distance_m - a.distance_m) / 1000, 1)} km · ${signed(b.ascent_m - a.ascent_m)} m climb · ${signed((b.seconds - a.seconds) / 60)} min · ${signed((unpaved(route) - unpaved(routes[0])) / 1000, 1)} km unpaved`;
    }
</script>

<div class="ways" role="radiogroup" aria-label="Ways">
    {#each routes as route (route.id)}
        <button type="button" role="radio" aria-checked={choiceId === route.id} onclick={() => onPick(route)}>
            <span class="mark"></span>
            <span class="what"><strong>{route.reason === 'corridor' ? 'Different corridor' : title(route.profile)}</strong><small>{(route.totals.surface_m[0] / 1000).toFixed(1)} km unknown surface · {Math.round(route.totals.seconds / 60)} min moving</small>{#if route.id !== routes[0].id}<small>{differences(route)}</small>{/if}</span>
            <span class="figure">{(route.totals.distance_m / 1000).toFixed(1)} km<small>{route.totals.unknown_elevation_m ? 'Elevation incomplete' : `↑ ${route.totals.ascent_m} m`}</small></span>
        </button>
    {/each}
    <p class="note" role="status">{status || (routes.length < 2 ? 'No useful alternative found.' : 'Alternatives offer a distance, surface, climbing or corridor trade-off.')}</p>
</div>

<style>
    .ways {
        padding: 4px 16px 16px;
    }
    button {
        display: flex;
        align-items: center;
        gap: 12px;
        width: calc(100% + 16px);
        margin: 0 -8px 4px;
        padding: 12px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
        text-align: left;
        font: inherit;
        cursor: pointer;
    }
    button:hover {
        background: var(--parchment-2);
    }
    .mark {
        width: 16px;
        height: 16px;
        flex: none;
        border: 1px solid var(--line-strong);
        border-radius: 50%;
    }
    [aria-checked="true"] .mark {
        border: 5px solid var(--ink);
    }
    .what {
        flex: 1;
        min-width: 0;
    }
    strong,
    small {
        display: block;
    }
    strong {
        font: 600 14px var(--sans);
    }
    small {
        margin-top: 2px;
        font: 400 13px var(--sans);
        color: var(--ink-soft);
    }
    .figure {
        text-align: right;
        font: 700 17px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .note {
        margin: 8px 0 0;
        font-size: 13px;
        color: var(--ink-soft);
    }
</style>
