<script lang="ts">
    let { value, min, max, axis, label, onResize }: {
        value: number;
        min: number;
        max: number;
        axis: 'x' | 'y';
        label: string;
        onResize: (value: number) => void;
    } = $props();

    let start: { position: number; value: number } | null = null;
    const steps: Record<string, number> = { ArrowLeft: -16, ArrowRight: 16, ArrowUp: 16, ArrowDown: -16 };

    function update(next: number) {
        onResize(Math.round(Math.max(min, Math.min(max, next))));
    }

    function position(event: PointerEvent) {
        return axis === 'x' ? event.clientX : event.clientY;
    }

    function down(event: PointerEvent) {
        event.preventDefault();
        (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
        start = { position: position(event), value };
    }

    function move(event: PointerEvent) {
        // The profile grows upwards, so a downward drag shrinks it.
        if (start) update(start.value + (position(event) - start.position) * (axis === 'x' ? 1 : -1));
    }

    function key(event: KeyboardEvent) {
        if (event.key in steps) update(value + steps[event.key]);
        else if (event.key === 'Home') update(min);
        else if (event.key === 'End') update(max);
        else return;
        event.preventDefault();
    }
</script>

<!-- A focusable separator implements the ARIA window splitter pattern. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
    role="separator"
    tabindex="0"
    aria-label={label}
    aria-orientation={axis === 'x' ? 'vertical' : 'horizontal'}
    aria-valuenow={value}
    aria-valuemin={min}
    aria-valuemax={max}
    class:vertical={axis === 'x'}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={() => start = null}
    onpointercancel={() => start = null}
    onkeydown={key}
></div>

<style>
    div {
        position: relative;
        flex: none;
        height: 8px;
        cursor: row-resize;
        touch-action: none;
        background: var(--panel);
    }
    div::after {
        content: "";
        position: absolute;
        inset: 3px 0;
        background: var(--line);
        transform: scaleY(.5);
    }
    .vertical {
        width: 8px;
        height: 100%;
        cursor: col-resize;
    }
    .vertical::after {
        inset: 0 3px;
        transform: scaleX(.5);
    }
    div:hover::after,
    div:focus-visible::after {
        background: var(--ink-soft);
        transform: none;
    }
    div:focus-visible {
        outline: none;
    }
</style>
