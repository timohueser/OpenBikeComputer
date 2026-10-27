<script lang="ts">
    import { onMount } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import TrailViewConditions from './TrailViewConditions.svelte';
    import TrailViewMiniMap from './TrailViewMiniMap.svelte';
    import { config } from '../../lib/planner/map-data';
    import type { Coordinate } from '../../lib/planner/geo';
    import { FIRST_HEADING, loadTrailView } from '../../lib/planner/trailview';
    import { nightPin, type RouteLook } from '../../lib/planner/trailview-route';
    import type { TrailView } from '../../lib/trailview/engine';
    import { VFOV } from '../../lib/trailview/frame/camera';
    import { defaultView, formatView, type View } from '../../lib/trailview/view';

    let { at, near, origin, onClose }: {
        at: Coordinate;
        near: import('../../lib/planner/trailview-first').Near;
        /** The viewport point that the window grows from. */
        origin: [number, number];
        onClose: () => void;
    } = $props();

    let dialog: HTMLDialogElement;
    let canvas: HTMLCanvasElement;
    let failure = $state('');
    let ground = $state(false);
    // The mini-map draws the basemap: its credits are the OSM data, its land cover and Protomaps.
    const creditsWith = (layers: string[]) => [...new Set([config.attribution, config.landcover_attribution ?? '', 'Protomaps', config.terrain_attribution, ...layers].filter(Boolean))];
    let credits = $state(creditsWith([]));
    let creditsOpen = $state(false);
    let creditsBox = $state<HTMLElement>();
    // A look-around drag that ends over the margin clicks the dialog too: only a press on the backdrop closes it.
    let pressedBackdrop = false;
    let engine: TrailView | undefined;
    let view = $state<View>();
    let width = $state(0), height = $state(0);
    // The plan's route draws on the terrain in the colours of its days, its grade or its surface, with a pin at
    // each night, until the rider hides it.
    const hasRoute = $derived(near.route.length > 1);
    let routeShown = $state(true), look = $state<RouteLook>('day');
    const drawRoute = () => engine?.showRoute(routeShown ? near.route : [], near.looks.colours[look],
        routeShown && hasRoute ? near.looks.nights.map(night => ({ coordinate: night.coordinate, image: nightPin(night) })) : []);
    const choose = (next: RouteLook) => { look = look === next ? 'day' : next; drawRoute(); };
    // The horizontal angle of the view, from the engine's vertical one and the window's shape.
    const fov = $derived(height ? 2 * Math.atan(Math.tan(VFOV * Math.PI / 360) * width / height) * 180 / Math.PI : VFOV);

    onMount(() => {
        dialog.showModal();
        // Only the transform animates: a new canvas size makes the engine's render targets again and restarts its frame history.
        const box = dialog.getBoundingClientRect();
        dialog.style.transformOrigin = `${origin[0] - box.left}px ${origin[1] - box.top}px`;
        const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
        dialog.animate(still ? [{ opacity: 0 }, { opacity: 1 }] : [{ transform: 'scale(0.04)' }, { transform: 'none' }],
            { duration: still ? 200 : 420, easing: 'cubic-bezier(.2,.8,.2,1)' });

        const abort = new AbortController();
        let stop: (() => void) | undefined;
        (async () => {
            const chunk = await loadTrailView();
            if (abort.signal.aborted) return;
            const started = await chunk.startTrailView(canvas, config, { ...defaultView([at[0], at[1], at[0], at[1]]), heading: FIRST_HEADING }, near);
            if (!started) {
                failure = 'gpu' in navigator ? 'TrailView cannot use the graphics card of this device.'
                    : 'TrailView needs WebGPU. Open the planner in a current version of Chrome, Edge or Safari.';
                return;
            }
            stop = () => {
                engine = view = undefined;
                started.destroy();
            };
            if (abort.signal.aborted) return stop();
            engine = started;
            drawRoute();
            view = started.view;
            started.onView = (next) => view = next;
            credits = creditsWith(started.attribution);
            if (await chunk.viewSettled(started, abort.signal)) ground = true;
            else if (!abort.signal.aborted) failure = 'TrailView could not load this place. Close the window and try again.';
        })().catch((error: unknown) => {
            console.error('TrailView:', error);
            failure = 'TrailView could not start. Close it and try again.';
        });
        return () => {
            abort.abort();
            stop?.();
        };
    });

    /** A new date rebuilds every ground page: the card sets one at most every 300 ms. */
    function change(conditions: Partial<View>) {
        if (!engine) return;
        engine.view = { ...engine.view, ...conditions };
        view = engine.view;
    }
</script>

<!-- A click on the backdrop lands on the dialog itself; its content fills it. Esc closes a modal dialog by itself.
     Esc and the Ctrl and Cmd shortcuts stop at the window: the planner below reads them on `window`, the engine reads none of them. -->
<dialog bind:this={dialog} bind:clientWidth={width} bind:clientHeight={height} class="window" aria-label="TrailView" onclose={onClose}
    onpointerdown={event => pressedBackdrop = event.target === dialog}
    onclick={event => {
        if (pressedBackdrop && event.target === dialog) dialog.close();
        else if (creditsOpen && !creditsBox?.contains(event.target as Node)) creditsOpen = false;
    }}
    onkeydown={event => { if (event.key === 'Escape' || event.ctrlKey || event.metaKey) event.stopPropagation(); }}
    oncopy={event => {
        // Ctrl+C copies the view as the TrailView page's query string, which that page takes with Ctrl+V.
        if (!engine || getSelection()?.toString()) return;
        event.clipboardData?.setData('text/plain', formatView(engine.view));
        event.preventDefault();
    }}>
    <!-- Focusable, so that after a click on the view the keys still pass through the dialog. -->
    <canvas bind:this={canvas} tabindex="-1"></canvas>
    <div class="cover" class:gone={ground && !failure}>
        {#if !failure}<p class="loading" role="status"><span class="spinner" aria-hidden="true"></span>Loading the view</p>{/if}
    </div>
    {#if failure}<p class="failure" role="alert">{failure}</p>
    {:else if view}
        <TrailViewMiniMap lat={view.lat} lon={view.lon} heading={view.heading} {fov} />
        <TrailViewConditions {view} onChange={change} />
        {#if hasRoute}
            <div class="route" role="group" aria-label="Route">
                <button type="button" aria-pressed={routeShown} onclick={() => { routeShown = !routeShown; drawRoute(); }}><Icon name="route" />Route</button>
                <button type="button" aria-pressed={look === 'grade'} disabled={!routeShown} onclick={() => choose('grade')}><Icon name="mountain" />Grade</button>
                <button type="button" aria-pressed={look === 'surface'} disabled={!routeShown} onclick={() => choose('surface')}><Icon name="layers" />Surface</button>
            </div>
        {/if}
    {/if}
    <button type="button" class="close" aria-label="Close TrailView" onclick={() => dialog.close()}><Icon name="close" /></button>
    {#if credits.length}
        <!-- Touch has no hover, so a tap toggles the credits. -->
        <div class="credits" class:open={creditsOpen} bind:this={creditsBox}>
            <small id="trailview-credits" role="tooltip">{credits.join(' · ')}</small>
            <button type="button" aria-label="Credits" aria-describedby="trailview-credits"
                onclick={event => { if ('pointerType' in event && event.pointerType === 'touch') creditsOpen = !creditsOpen; }}>
                <svg viewBox="0 0 20 20" aria-hidden="true"><path fill="currentColor" fill-rule="evenodd" d="M4 10a6 6 0 1 0 12 0 6 6 0 1 0-12 0m5-3a1 1 0 1 0 2 0 1 1 0 1 0-2 0m0 3a1 1 0 1 1 2 0v3a1 1 0 1 1-2 0" /></svg>
            </button>
        </div>
    {/if}
</dialog>

<style>
    .window {
        /* The engine's clear colour before the ground arrives: sunny weather's horizon. */
        --haze: #b4bcc4;
        position: fixed;
        inset: 32px;
        width: auto;
        height: auto;
        max-width: none;
        max-height: none;
        margin: 0;
        padding: 0;
        border: 0;
        border-radius: 14px;
        overflow: hidden;
        background: var(--haze);
        color: var(--ink);
        font-family: var(--sans);
        box-shadow: 0 18px 48px rgb(28 27 20 / 0.35);
    }
    .window::backdrop {
        background: rgb(28 27 20 / 0.55);
        animation: fade 420ms cubic-bezier(.2, .8, .2, 1);
    }
    :global([data-theme="dark"]) .window::backdrop {
        background: rgb(0 0 0 / 0.55);
    }
    @keyframes fade {
        from { opacity: 0; }
    }
    @media (max-width: 760px) {
        .window {
            inset: 0;
            border-radius: 0;
        }
    }
    canvas {
        display: block;
        width: 100%;
        height: 100%;
        outline: none;
        touch-action: none;
        cursor: grab;
    }
    /* The cover takes the pointer while it shows: a look around under it would turn the view the rider has not seen. */
    .cover {
        position: absolute;
        inset: 0;
        display: grid;
        place-items: center;
        background: var(--haze);
        transition: opacity 400ms ease-out;
    }
    .cover.gone {
        opacity: 0;
        pointer-events: none;
    }
    .loading {
        display: flex;
        align-items: center;
        gap: 10px;
        margin: 0;
        color: #1c1b14;
        font: 600 14px var(--sans);
    }
    .spinner {
        width: 18px;
        height: 18px;
        border: 2.5px solid rgba(28, 27, 20, .25);
        border-top-color: #1c1b14;
        border-radius: 50%;
        animation: spin 900ms linear infinite;
    }
    @keyframes spin {
        to { transform: rotate(1turn); }
    }
    @media (prefers-reduced-motion: reduce) {
        .spinner { animation-duration: 3s; }
    }
    .failure {
        position: absolute;
        inset: 0;
        display: grid;
        place-content: center;
        margin: 0;
        padding: 24px;
        background: var(--panel);
        font-size: 16px;
        text-align: center;
    }
    .close {
        position: absolute;
        top: max(12px, env(safe-area-inset-top));
        right: max(12px, env(safe-area-inset-right));
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        padding: 0;
        border: 0;
        border-radius: 50%;
        background: var(--panel);
        color: var(--ink);
        box-shadow: var(--planner-shadow);
        cursor: pointer;
    }
    .close:hover {
        color: var(--link);
    }
    .route {
        position: absolute;
        top: max(12px, env(safe-area-inset-top));
        right: calc(max(12px, env(safe-area-inset-right)) + 44px);
        display: flex;
        height: 36px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        overflow: hidden;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    .route button {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 0 12px;
        border: 0;
        background: none;
        color: var(--ink-soft);
        font: 700 14px var(--sans);
        cursor: pointer;
    }
    .route button + button {
        border-left: 1px solid var(--line-strong);
    }
    .route button[aria-pressed="true"] {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .route button:disabled {
        opacity: 0.45;
        cursor: default;
    }
    .route button:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: -2px;
    }
    .credits {
        position: absolute;
        right: max(10px, env(safe-area-inset-right));
        bottom: max(10px, env(safe-area-inset-bottom));
        display: flex;
        flex-direction: column;
        align-items: flex-end;
        gap: 4px;
    }
    .credits small {
        display: none;
        max-width: min(320px, 70vw);
        padding: 3px 8px;
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink-soft);
        font: 11px var(--sans);
    }
    .credits button {
        display: grid;
        place-items: center;
        width: 24px;
        height: 24px;
        padding: 0;
        border: 0;
        border-radius: 12px;
        background: var(--panel);
        color: var(--ink);
    }
    /* A tap leaves :hover and :focus on the button: only a real hover and keyboard focus show the credits. */
    .credits:is(.open, :has(:focus-visible)) small {
        display: block;
    }
    @media (hover: hover) {
        .credits:hover small {
            display: block;
        }
    }
    .credits svg {
        width: 24px;
        height: 24px;
    }
</style>
