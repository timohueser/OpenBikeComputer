<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { DRAG_PX, dropPoint, loadTrailView, settle, swing } from '../../lib/planner/trailview';

    let { active, onActivate, onDrag, onDrop }: {
        /** The next map click opens TrailView. */
        active: boolean;
        /** A click, Enter or Space without a drag. */
        onActivate: () => void;
        onDrag: (dragging: boolean) => void;
        /** The drop point in the viewport; true when TrailView opens there. */
        onDrop: (x: number, y: number) => boolean;
    } = $props();

    let button: HTMLButtonElement;
    let hanging = $state<HTMLElement>();
    let press: { id: number; x: number; y: number } | null = null;
    let drag = $state<{ x: number; y: number; angle: number; returning: boolean } | null>(null);
    // The click that follows a drop or a cancelled drag is no activation.
    let dragged = false;
    let frame = 0;
    const drop = $derived(drag && dropPoint([drag.x, drag.y], drag.angle));

    export function focus() {
        button.focus();
    }

    function down(event: PointerEvent) {
        dragged = false;
        if (event.button !== 0 || drag) return;
        // The engine loads while the rider drags.
        loadTrailView().catch(() => {});
        press = { id: event.pointerId, x: event.clientX, y: event.clientY };
        button.setPointerCapture(event.pointerId);
    }

    function move(event: PointerEvent) {
        if (press?.id !== event.pointerId) return;
        if (!drag) {
            if (Math.hypot(event.clientX - press.x, event.clientY - press.y) < DRAG_PX) return;
            drag = { x: press.x, y: press.y, angle: 0, returning: false };
            onDrag(true);
            let last = performance.now();
            const rest = (now: number) => {
                if (!drag || drag.returning) return;
                drag.angle = settle(drag.angle, now - last);
                last = now;
                frame = requestAnimationFrame(rest);
            };
            frame = requestAnimationFrame(rest);
        }
        drag = { ...drag, x: event.clientX, y: event.clientY, angle: swing(drag.angle, event.clientX - drag.x) };
    }

    function up(event: PointerEvent) {
        if (press?.id !== event.pointerId) return;
        press = null;
        if (!drag || !drop) return;
        dragged = true;
        end(onDrop(...drop));
    }

    function cancel() {
        if (!press) return;
        const { id } = press;
        press = null;
        if (button.hasPointerCapture(id)) button.releasePointerCapture(id);
        if (!drag) return;
        dragged = true;
        end(false);
    }

    /** Ends a drag: the binoculars go back to the control unless TrailView opened. */
    function end(opened: boolean) {
        cancelAnimationFrame(frame);
        onDrag(false);
        if (opened || !drag || !hanging || matchMedia('(prefers-reduced-motion: reduce)').matches) {
            drag = null;
            return;
        }
        drag.returning = true;
        const box = button.getBoundingClientRect();
        const home = `translate(${box.left + box.width / 2}px, ${box.top + 4}px) translateX(-50%) rotate(0deg) scale(0.6)`;
        hanging.animate([{ transform: hanging.style.transform }, { transform: home, opacity: 0.3 }], { duration: 240, easing: 'cubic-bezier(.2,.8,.2,1)' })
            .finished.then(() => drag = null, () => drag = null);
    }
</script>

<svelte:window onkeydown={event => { if (event.key === 'Escape' && drag && !drag.returning) cancel(); }} />

<div class="control">
    <button type="button" bind:this={button} class:chosen={active} aria-pressed={active} aria-label="Drag onto the map to open TrailView" title="Drag onto the map to open TrailView"
        onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={cancel} onlostpointercapture={cancel}
        onclick={() => { if (dragged) dragged = false; else onActivate(); }}>
        <Icon name="binoculars" />
    </button>
</div>
{#if drag}
    {#if drop && !drag.returning}<div class="shadow" style:transform={`translate(${drop[0]}px, ${drop[1]}px) translate(-50%, -50%)`}></div>{/if}
    <div class="hanging" bind:this={hanging} style:transform={`translate(${drag.x}px, ${drag.y}px) translateX(-50%) rotate(${drag.angle}deg)`}>
        <Icon name="binoculars" size={22} />
    </div>
{/if}

<style>
    .control {
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    button {
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        padding: 0;
        border: 0;
        border-radius: 8px;
        background: none;
        color: var(--ink);
        cursor: grab;
        touch-action: none;
    }
    button:hover,
    button.chosen {
        color: var(--link);
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
