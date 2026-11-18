<script module lang="ts">
    import type { Coordinate } from '../../lib/planner/geo';
    import type { Profile } from '../../lib/trailview/route/profile';

    /** A route of the routes mode, with the heights along it and the credit of its source. */
    export type TrailViewRoute = { id: string; line: Coordinate[]; profile?: Profile; colour: string; credit?: string };
</script>

<script lang="ts">
    import { onMount, untrack, type Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import TrailViewConditions from './TrailViewConditions.svelte';
    import TrailViewMiniMap from './TrailViewMiniMap.svelte';
    import TrailViewProfile from './TrailViewProfile.svelte';
    import { config } from '../../lib/planner/map-data';
    import { creditsWith } from '../../lib/planner/map-setup';
    import { recordCredits } from '../../lib/planner/signed-routes';
    import Credits from './Credits.svelte';
    import { coordinateAt, cumulative } from '../../lib/planner/geo';
    import { FIRST_HEADING, loadTrailView } from '../../lib/planner/trailview';
    import type { Near } from '../../lib/planner/trailview-first';
    import { nightBadge, type RouteLook } from '../../lib/planner/trailview-route';
    import { sampleIndex } from '../../lib/planner/profile-data';
    import type { TrailView } from '../../lib/trailview/engine';
    import { VFOV } from '../../lib/trailview/frame/camera';
    import { defaultView, formatView, type View } from '../../lib/trailview/view';

    type None<T> = { [K in keyof T]?: undefined };
    type Drop = { at: Coordinate; near: Near };
    type Routes = {
        routes: TrailViewRoute[];
        /** The first view. */
        view: View;
        /** The id of the route that is bold while the pointer is on no route. */
        selected?: string;
        /** The id of the route under a click, or undefined after a click where no route is. */
        onselect?: (id: string | undefined) => void;
        /** Shows at the bottom of the window, over the view or its notice. It gets a function that moves the focus to
         * the view: a card that closes itself calls it first, because its close button goes with it. */
        card?: Snippet<[() => void]>;
    };
    // The window has two modes: a drop at `at` with the plan's data `near`, or `routes` from the first `view`.
    let props: {
        /** The viewport point that the window grows from. */
        origin: [number, number];
        onClose: () => void;
    } & ((Drop & None<Routes>) | (Routes & None<Drop>)) = $props();

    let dialog: HTMLDialogElement;
    let canvas: HTMLCanvasElement;
    let failure = $state('');
    let ground = $state(false);
    // A look-around drag that ends over the margin clicks the dialog too: only a press on the backdrop closes it.
    let pressedBackdrop = false;
    let engine: TrailView | undefined;
    let view = $state<View>();
    let width = $state(0), height = $state(0);
    // The window keeps what it opens with: route number i + 1 of the engine is routes[i].
    const opened = untrack(() => ({ ...props })), { near, routes } = opened;
    const routeCredits = recordCredits(routes ?? []);
    let credits = $state(creditsWith(routeCredits));
    // The plan's route draws on the terrain in the colours of its days, its grade or its surface, with a pin at
    // each night and a marker at the point of the profile under the pointer, until the rider hides it.
    const hasRoute = !!near && near.route.length > 1;
    const line = [...near?.route ?? []];
    let routeShown = $state(true), look = $state<RouteLook>('day'), position = $state<number | null>(null);
    const nightPins = near?.overlay.nights.map(night => ({ coordinate: night.coordinate, colour: night.color, badge: nightBadge(night) })) ?? [];
    // The route under the pointer is bold; with none, the selected route.
    let hovered: number | null = null, bolded: number | null | undefined;
    const bold = () => {
        if (!routes || !engine) return;
        const number = hovered ?? (routes.findIndex(route => route.id === props.selected) + 1 || null);
        if (number === bolded) return;
        bolded = number;
        engine.boldRoute(number);
    };
    $effect(() => {
        void props.selected;
        untrack(bold);
    });
    const pointer = routes && {
        onhover: (x: number, y: number) => { hovered = engine?.routeAt(x, y) ?? null; bold(); },
        onclick: (x: number, y: number) => {
            const number = engine?.routeAt(x, y);
            props.onselect?.(number ? routes[number - 1].id : undefined);
        },
    };
    const drawRoute = () => {
        if (routes) {
            engine?.showRoutes(routes.map(route => ({ line: route.line, profile: route.profile, colours: route.line.slice(1).map(() => route.colour) })));
            bolded = undefined;
            bold();
            return;
        }
        if (!near) return;
        engine?.showRoutes(routeShown ? [{ line: near.route, colours: near.overlay.colours[look] }] : []);
        engine?.showPins(routeShown && hasRoute ? nightPins : []);
        drawMarker();
    };
    // The marker takes the colour of the edge it stands on.
    const drawMarker = () => {
        if (!near) return;
        const colours = near.overlay.colours[look], edge = Math.max(sampleIndex(near.overlay.profile, position ?? 0) - 1, 0);
        engine?.showMarker(!routeShown || position === null ? null : coordinateAt(line, position), colours[Math.min(edge, colours.length - 1)]);
    };
    const choose = (next: RouteLook) => { look = look === next ? 'day' : next; drawRoute(); };
    // The horizontal angle of the view, from the engine's vertical one and the window's shape.
    const fov = $derived(height ? 2 * Math.atan(Math.tan(VFOV * Math.PI / 360) * width / height) * 180 / Math.PI : VFOV);

    onMount(() => {
        dialog.showModal();
        // Only the transform animates: a new canvas size makes the engine's render targets again and restarts its frame history.
        const box = dialog.getBoundingClientRect();
        dialog.style.transformOrigin = `${props.origin[0] - box.left}px ${props.origin[1] - box.top}px`;
        const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
        dialog.animate(still ? [{ opacity: 0 }, { opacity: 1 }] : [{ transform: 'scale(0.04)' }, { transform: 'none' }],
            { duration: still ? 200 : 420, easing: 'cubic-bezier(.2,.8,.2,1)' });

        const abort = new AbortController();
        let stop: (() => void) | undefined;
        (async () => {
            const chunk = await loadTrailView();
            if (abort.signal.aborted) return;
            const start = opened.routes ? opened.view : { ...defaultView([opened.at[0], opened.at[1], opened.at[0], opened.at[1]]), heading: FIRST_HEADING };
            const started = await chunk.startTrailView(canvas, config, start, near, abort.signal, pointer);
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
            credits = creditsWith([...started.attribution, ...routeCredits]);
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
<dialog bind:this={dialog} bind:clientWidth={width} bind:clientHeight={height} class="window" aria-label="TrailView" onclose={props.onClose}
    onpointerdown={event => pressedBackdrop = event.target === dialog}
    onclick={event => {
        if (pressedBackdrop && event.target === dialog) dialog.close();
    }}
    onkeydown={event => { if (event.key === 'Escape' || event.ctrlKey || event.metaKey) event.stopPropagation(); }}
    oncopy={event => {
        // Ctrl+C copies the view as the TrailView page's query string, which that page takes with Ctrl+V.
        if (!engine || getSelection()?.toString()) return;
        event.clipboardData?.setData('text/plain', formatView(engine.view));
        event.preventDefault();
    }}>
    <!-- Focusable, so that after a click on the view the keys still pass through the dialog. -->
    <canvas bind:this={canvas} tabindex="-1" onpointerleave={() => { if (routes) { hovered = null; bold(); } }}></canvas>
    <div class="cover" class:gone={ground && !failure}>
        {#if !failure}<p class="loading" role="status"><span class="spinner" aria-hidden="true"></span>Loading the view</p>{/if}
    </div>
    {#if failure}<p class="failure" role="alert">{failure}</p>
    {:else if view}
        <TrailViewMiniMap lat={view.lat} lon={view.lon} heading={view.heading} {fov} />
        <TrailViewConditions {view} onChange={change} />
        {#if hasRoute}
            <div class="route" role="group" aria-label="Route">
                <button type="button" aria-pressed={routeShown} onclick={() => { routeShown = !routeShown; position = null; drawRoute(); }}><Icon name="route" />Route</button>
                <button type="button" aria-pressed={look === 'grade'} disabled={!routeShown} onclick={() => choose('grade')}><Icon name="mountain" />Grade</button>
                <button type="button" aria-pressed={look === 'surface'} disabled={!routeShown} onclick={() => choose('surface')}><Icon name="layers" />Surface</button>
            </div>
            {#if routeShown && near}
                <TrailViewProfile profile={near.overlay.profile} colours={near.overlay.colours[look]} total={cumulative(line).at(-1) ?? 0}
                    onHover={progress => { position = progress; drawMarker(); }} />
            {/if}
        {/if}
    {/if}
    {#if props.card}<div class="card">{@render props.card(() => canvas.focus())}</div>{/if}
    <button type="button" class="close" aria-label="Close TrailView" onclick={() => dialog.close()}><Icon name="close" /></button>
    <Credits {credits} />
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
    /* The card takes the place of the profile: right of the small map, left of the credits. */
    .card {
        position: absolute;
        left: calc(max(12px, env(safe-area-inset-left)) + 232px);
        right: calc(max(12px, env(safe-area-inset-right)) + 44px);
        bottom: max(12px, env(safe-area-inset-bottom));
    }
    @media (max-width: 640px) {
        .card {
            left: max(12px, env(safe-area-inset-left));
            right: max(12px, env(safe-area-inset-right));
            bottom: calc(max(12px, env(safe-area-inset-bottom)) + 152px);
            /* The room between the controls at the top and the small map: a long text scrolls inside the card. */
            --card-height: calc(100dvh - 236px - env(safe-area-inset-top) - env(safe-area-inset-bottom));
        }
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
</style>
