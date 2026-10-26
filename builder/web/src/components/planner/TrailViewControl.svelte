<script lang="ts">
    import { onDestroy } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import { dragPx, dropPoint, loadTrailView, settle, swing } from '../../lib/planner/trailview';

    let { active, onActivate, onDrop }: {
        /** The next map click opens TrailView. */
        active: boolean;
        /** A click, Enter or Space without a drag. */
        onActivate: () => void;
        /** The drop point in the viewport; true when TrailView opens there. */
        onDrop: (x: number, y: number) => boolean;
    } = $props();

    let button: HTMLButtonElement;
    let hanging = $state<HTMLElement>();
    let press: { id: number; x: number; y: number; slop: number } | null = null;
    let drag = $state<{ x: number; y: number; angle: number; returning: boolean } | null>(null);
    let frame = 0;
    const drop = $derived(drag && dropPoint([drag.x, drag.y], drag.angle));
    const still = matchMedia('(prefers-reduced-motion: reduce)');

    export function focus() {
        button.focus();
    }

    onDestroy(() => cancelAnimationFrame(frame));

    function down(event: PointerEvent) {
        if (event.button !== 0 || (drag && !drag.returning)) return;
        // A new press ends the swing back to the control.
        drag = null;
        // The engine loads while the rider drags.
        loadTrailView().then(chunk => chunk.requestGpu()).catch(() => {});
        press = { id: event.pointerId, x: event.clientX, y: event.clientY, slop: dragPx(event.pointerType) };
        button.setPointerCapture(event.pointerId);
    }

    function move(event: PointerEvent) {
        if (press?.id !== event.pointerId) return;
        if (!drag) {
            if (Math.hypot(event.clientX - press.x, event.clientY - press.y) < press.slop) return;
            drag = { x: press.x, y: press.y, angle: 0, returning: false };
            let last = performance.now();
            const rest = (now: number) => {
                if (!drag || drag.returning) return;
                drag.angle = settle(drag.angle, now - last);
                last = now;
                frame = requestAnimationFrame(rest);
            };
            if (!still.matches) frame = requestAnimationFrame(rest);
        }
        drag = { ...drag, x: event.clientX, y: event.clientY, angle: still.matches ? 0 : swing(drag.angle, event.clientX - drag.x) };
    }

    // The press decides: a release over the button is a click, also after a wobble; a release elsewhere is a drop.
    function up(event: PointerEvent) {
        if (press?.id !== event.pointerId) return;
        press = null;
        const box = button.getBoundingClientRect();
        const over = event.clientX >= box.left && event.clientX <= box.right && event.clientY >= box.top && event.clientY <= box.bottom;
        if (!drag || over) {
            if (drag) end(true);
            onActivate();
        } else if (drop) {
            end(onDrop(...drop));
        }
    }

    function cancel() {
        if (!press) return;
        const { id } = press;
        press = null;
        if (button.hasPointerCapture(id)) button.releasePointerCapture(id);
        if (drag) end(false);
    }

    /** Ends a drag: the binoculars swing back to the control, or vanish at once when `now`. */
    function end(now: boolean) {
        cancelAnimationFrame(frame);
        if (now || !drag || !hanging || still.matches) {
            drag = null;
            return;
        }
        drag.returning = true;
        const box = button.getBoundingClientRect();
        const back = `translate(${box.left + box.width / 2}px, ${box.top + 4}px) translateX(-50%) rotate(0deg) scale(0.6)`;
        hanging.animate([{ transform: hanging.style.transform }, { transform: back, opacity: 0.3 }], { duration: 240, easing: 'cubic-bezier(.2,.8,.2,1)' })
            .finished.finally(() => { if (drag?.returning) drag = null; });
    }
</script>

<!-- The capture phase on window comes before the planner's Esc handler, so Esc in a drag only ends the drag. -->
<svelte:window onkeydowncapture={event => { if (event.key === 'Escape' && drag && !drag.returning) { event.stopPropagation(); cancel(); } }} onblur={cancel} />

<!-- A pointer activates on its release (see `up`); a click with detail 0 comes from Enter or Space. -->
<button type="button" bind:this={button} class:chosen={active} aria-pressed={active} aria-label="Drag onto the map to open TrailView" title="Drag onto the map to open TrailView"
    onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={cancel} onlostpointercapture={cancel}
    onclick={event => { if (event.detail === 0) onActivate(); }}>
    <Icon name="binoculars" />
</button>
{#if drag}
    {#if drop && !drag.returning}<div class="shadow" style:transform={`translate(${drop[0]}px, ${drop[1]}px) translate(-50%, -50%)`}></div>{/if}
    <div class="hanging" bind:this={hanging} style:transform={`translate(${drag.x}px, ${drag.y}px) translateX(-50%) rotate(${drag.angle}deg)`}>
        <Icon name="binoculars" size={22} />
    </div>
{/if}

<style>
    button {
        cursor: grab;
        touch-action: none;
    }
    /* The top centre of the binoculars hangs at the pointer and is their pivot; their bottom is the drop point. */
    .hanging {
        position: fixed;
        top: 0;
        left: 0;
        z-index: 20;
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
        box-shadow: var(--planner-shadow);
        transform-origin: 50% 0;
        pointer-events: none;
    }
    .shadow {
        position: fixed;
        top: 0;
        left: 0;
        z-index: 19;
        width: 22px;
        height: 7px;
        border-radius: 50%;
        background: radial-gradient(closest-side, rgb(28 27 20 / 0.45), rgb(28 27 20 / 0));
        pointer-events: none;
    }
</style>
