<!-- Getting there: a line from the start point through the ways to travel that the rider ticks, Car, Train (later) and
     Lifts, to one walk to the route. With no start point every hike of the map view counts, and only Lifts shows: it adds
     the lift shapes. A pinned car park or top station is the only start. -->
<script lang="ts">
    import type { FactsPlace } from '../../lib/planner/signed-routes';
    import Icon from '../../lib/ui/Icon.svelte';
    import { DRIVE_KM, SHAPES, type Query } from './search';
    import { placeName, shortDistance } from './words';

    let { query, cap, place, onchange }: {
        query: Query;
        /** The longest drive of the start point. */
        cap: number;
        place: (id: string) => FactsPlace | undefined;
        onchange: (change: Partial<Query>) => void;
    } = $props();

    /** The stops of the walk slider, in metres. */
    const WALKS = [200, 300, 500, 750, 1000, 1500, 2000, 3000];
    const walkAt = $derived(WALKS.reduce((best, metres, i) => Math.abs(metres - query.walk) < Math.abs(WALKS[best] - query.walk) ? i : best, 0));
    const pinned = $derived(query.pin ? place(query.pin) : undefined);
    const from = $derived(!!query.start || !!query.pin);
    const drive = $derived(Math.min(query.drive, cap));
    /** Lifts off hides the lift shapes: with neither loop nor there and back on, both come on again. */
    function lifts() {
        if (!query.lift) return onchange({ lift: 'helps' });
        const ground = (shape: string) => shape === 'loop' || shape === 'back';
        onchange({ lift: undefined, ...query.shapes.some(ground) ? {} : { shapes: SHAPES.filter(shape => ground(shape) || query.shapes.includes(shape)) } });
    }
    const walkNote = $derived(query.pin ? `From the ${pinned?.kind === 'lift' ? 'top station' : 'car park'} to the route`
        : query.car || query.lift ? 'The last part, to the route' : 'From your start to the route');
</script>

<section>
    <div class="head"><h3>Getting there</h3></div>
    <ol class="journey" class:alone={!from}>
        <li class="leg start">
            {#if query.pin}
                <div class="leg-head">
                    <span class="node dot"></span>{#if pinned?.kind === 'lift'}<Icon name="lift" />{:else}<span class="park">P</span>{/if}
                    <span class="label">{placeName(pinned)}<small>Pinned on the map</small></span>
                    <button type="button" class="text" onclick={() => onchange({ pin: undefined })}>Unpin</button>
                </div>
            {:else if query.start}
                <div class="leg-head">
                    <span class="node dot"></span><Icon name="pin" />
                    <span class="label">Start point<small>{query.start[1].toFixed(3)}° N, {query.start[0].toFixed(3)}° E</small></span>
                    <button type="button" class="remove" aria-label="Remove the start point" onclick={() => onchange({ start: undefined })}><Icon name="close" size={16} /></button>
                </div>
            {:else}
                <p class="hint">No start point: every hike in the map view counts.</p>
            {/if}
        </li>
        {#if query.start && !query.pin}
            <li class="leg" class:on={query.car}>
                <button type="button" class="leg-head" aria-pressed={query.car} onclick={() => onchange({ car: !query.car })}>
                    <span class="node box"><Icon name="check" size={12} /></span><Icon name="car" />
                    <span class="label">Car<small>to a car park or a lift</small></span>
                    {#if query.car}<b class="value">up to {drive} km</b>{/if}
                </button>
                {#if query.car}
                    <div class="leg-body">
                        <input type="range" min={DRIVE_KM.min} max={cap} step={DRIVE_KM.step} value={drive} aria-label="Longest drive, one way" aria-valuetext="up to {drive} km"
                            oninput={event => onchange({ drive: Number(event.currentTarget.value) })} />
                    </div>
                {/if}
            </li>
            <li class="leg later">
                <div class="leg-head"><span class="node box"></span><Icon name="train" /><span class="label">Train<small>Later: needs timetables</small></span></div>
            </li>
        {/if}
        {#if !query.pin}
            <li class="leg" class:on={!!query.lift}>
                <button type="button" class="leg-head" aria-pressed={!!query.lift} onclick={lifts}>
                    <span class="node box"><Icon name="check" size={12} /></span><Icon name="lift" />
                    <span class="label">Lifts<small>cable cars, gondolas, chairlifts</small></span>
                    {#if query.lift === 'always'}<b class="value">always</b>{/if}
                </button>
                {#if query.lift}
                    <div class="leg-body">
                        {#if from}
                            <div class="when"><span>Use a lift</span>
                                <span class="choice" role="group" aria-label="Use a lift">
                                    <button type="button" aria-pressed={query.lift === 'helps'} onclick={() => onchange({ lift: 'helps' })}>When it helps</button>
                                    <button type="button" aria-pressed={query.lift === 'always'} onclick={() => onchange({ lift: 'always' })}>Always</button>
                                </span>
                            </div>
                        {/if}
                        <p class="hint"><Icon name="info" size={14} />We do not know when a lift runs.</p>
                    </div>
                {/if}
            </li>
        {/if}
        {#if from}
            <li class="leg walk">
                <div class="leg-head">
                    <span class="node walked"></span><Icon name="walk" />
                    <span class="label">Walk<small>{walkNote}</small></span>
                    <b class="value">up to {shortDistance(query.walk)}</b>
                </div>
                <div class="leg-body">
                    <input type="range" min="0" max={WALKS.length - 1} step="1" value={walkAt} aria-label="Longest walk to the route" aria-valuetext="up to {shortDistance(query.walk)}"
                        oninput={event => onchange({ walk: WALKS[Number(event.currentTarget.value)] })} />
                </div>
            </li>
            <li class="leg end"><div class="leg-head"><span class="node ring"></span><span></span><span class="label">The route</span></div></li>
        {/if}
    </ol>
</section>

<style>
    .journey {
        display: flex;
        flex-direction: column;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    /* Each node sits on one line that joins the legs. */
    .leg {
        position: relative;
    }
    .leg::before {
        content: "";
        position: absolute;
        top: 0;
        bottom: 0;
        left: 8px;
        width: 2px;
        background: var(--parchment-3);
    }
    .start::before {
        top: 22px;
    }
    .end::before {
        bottom: auto;
        height: 16px;
    }
    .alone .leg::before {
        display: none;
    }
    .leg-head {
        display: grid;
        position: relative;
        grid-template-columns: 18px 18px minmax(0, 1fr) auto;
        align-items: center;
        gap: 10px;
        width: 100%;
        min-height: 44px;
        padding: 4px 0;
        border: 0;
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
    }
    .leg-head > :global(svg), .park {
        justify-self: center;
        color: var(--ink-soft);
    }
    .label {
        display: flex;
        flex-direction: column;
        min-width: 0;
        font-size: 13.5px;
        font-weight: 600;
        line-height: 1.3;
    }
    small {
        color: var(--ink-soft);
        font-size: 12.5px;
        font-weight: 400;
    }
    .value {
        font-size: 13px;
        font-variant-numeric: tabular-nums;
    }
    .node {
        position: relative;
        z-index: 1;
        display: grid;
        place-items: center;
        width: 18px;
        height: 18px;
        border-radius: 50%;
        background: var(--panel);
    }
    .box {
        border: 1.5px solid var(--line-strong);
        border-radius: 5px;
        color: var(--panel);
    }
    .box :global(svg) {
        transform: scale(0);
        transition: transform 180ms cubic-bezier(0.23, 1, 0.32, 1);
        stroke-width: 3;
    }
    .leg-head:hover .box {
        border-color: var(--ink-soft);
    }
    [aria-pressed="true"] > .box {
        border-color: var(--ink);
        background: var(--ink);
    }
    [aria-pressed="true"] > .box :global(svg) {
        transform: scale(1);
    }
    .dot::after {
        content: "";
        width: 10px;
        height: 10px;
        border-radius: 50%;
        background: var(--ink);
    }
    .ring::after {
        content: "";
        width: 10px;
        height: 10px;
        border: 2px solid var(--ink-soft);
        border-radius: 50%;
    }
    /* The walk is always on: a filled box with a tick. */
    .walked {
        border-radius: 5px;
        background: var(--ink-soft);
    }
    .walked::after {
        content: "";
        width: 8px;
        height: 4px;
        margin-top: -2px;
        border: solid var(--panel);
        border-width: 0 0 2px 2px;
        transform: rotate(-45deg);
    }
    .later .label {
        color: var(--ink-soft);
        font-weight: 500;
    }
    .later .box {
        border-style: dashed;
    }
    .end .leg-head {
        min-height: 32px;
    }
    .end .label {
        color: var(--ink-soft);
        font-weight: 500;
    }
    .leg-body {
        display: flex;
        flex-direction: column;
        align-items: stretch;
        gap: 10px;
        padding: 0 0 12px 56px;
    }
    .alone .leg-body {
        padding-left: 28px;
    }
    input[type="range"] {
        width: 100%;
        margin: 0;
        accent-color: var(--ink-soft);
    }
    .when {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 6px 10px;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .choice {
        display: inline-flex;
        gap: 2px;
        padding: 2px;
        border-radius: 8px;
        background: var(--parchment-2);
    }
    .choice button {
        min-height: 28px;
        padding: 0 10px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: 600 12.5px var(--sans);
    }
    .choice button:hover {
        color: var(--ink);
    }
    .choice [aria-pressed="true"] {
        background: var(--panel);
        color: var(--ink);
        box-shadow: 0 0 0 1px var(--line-strong);
    }
    .park {
        display: inline-grid;
        place-items: center;
        width: 16px;
        height: 16px;
        border-radius: 4px;
        background: var(--water);
        color: var(--panel);
        font: 700 10.5px/1 var(--sans);
    }
    .text, .remove {
        padding: 4px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: 600 13px var(--sans);
    }
    .remove {
        display: grid;
        place-items: center;
        width: 30px;
        height: 30px;
        padding: 0;
    }
    .text:hover, .remove:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .start .hint {
        padding: 2px 0 6px;
    }
</style>
