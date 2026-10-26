<script lang="ts">
    import { onMount } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import { config } from '../../lib/planner/map-data';
    import type { Coordinate } from '../../lib/planner/geo';
    import { FIRST_HEADING, loadTrailView } from '../../lib/planner/trailview';
    import { defaultView } from '../../lib/trailview/view';

    let { at, origin, onClose }: {
        at: Coordinate;
        /** The viewport point that the window grows from. */
        origin: [number, number];
        onClose: () => void;
    } = $props();

    let dialog: HTMLDialogElement;
    let canvas: HTMLCanvasElement;
    let failure = $state('');
    let ground = $state(false);
    let credits = $state([config.terrain_attribution].filter(Boolean));
    let creditsOpen = $state(false);
    let creditsBox = $state<HTMLElement>();
    // A look-around drag that ends over the margin clicks the dialog too: only a press on the backdrop closes it.
    let pressedBackdrop = false;

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
            const engine = await chunk.startTrailView(canvas, config, { ...defaultView([at[0], at[1], at[0], at[1]]), heading: FIRST_HEADING });
            if (!engine) {
                failure = 'gpu' in navigator ? 'TrailView cannot use the graphics card of this device.'
                    : 'TrailView needs WebGPU. Open the planner in a current version of Chrome, Edge or Safari.';
                return;
            }
            stop = () => chunk.stopTrailView(engine);
            if (abort.signal.aborted) return stop();
            credits = [config.terrain_attribution, ...engine.attribution].filter(Boolean);
            if (await chunk.groundDrawn(engine, abort.signal)) ground = true;
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
</script>

<!-- A click on the backdrop lands on the dialog itself; its content fills it. Esc closes a modal dialog by itself.
     Esc and the Ctrl and Cmd shortcuts stop at the window: the planner below reads them on `window`, the engine reads none of them. -->
<dialog bind:this={dialog} class="window" aria-label="TrailView" onclose={onClose}
    onpointerdown={event => pressedBackdrop = event.target === dialog}
    onclick={event => {
        if (pressedBackdrop && event.target === dialog) dialog.close();
        else if (creditsOpen && !creditsBox?.contains(event.target as Node)) creditsOpen = false;
    }}
    onkeydown={event => { if (event.key === 'Escape' || event.ctrlKey || event.metaKey) event.stopPropagation(); }}>
    <!-- Focusable, so that after a click on the view the keys still pass through the dialog. -->
    <canvas bind:this={canvas} tabindex="-1"></canvas>
    <div class="cover" class:gone={ground && !failure} aria-hidden="true"></div>
    {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
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
    .cover {
        position: absolute;
        inset: 0;
        background: var(--haze);
        transition: opacity 900ms ease-out;
        pointer-events: none;
    }
    .cover.gone {
        opacity: 0;
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
