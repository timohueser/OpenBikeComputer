<script module lang="ts">
    /** Arrow keys choose the next or previous radio of a radio group, wrapping at the ends, and focus it. */
    export function radioKey(event: KeyboardEvent, index: number, count: number, choose: (index: number) => void): void {
        const step = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 0;
        if (!step) return;
        event.preventDefault();
        const next = (index + step + count) % count;
        choose(next);
        (event.currentTarget as HTMLElement).closest('[role="radiogroup"]')?.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus();
    }
</script>

<script lang="ts" generics="T extends string">
    import Icon from './Icon.svelte';

    let { label, options, value, onChange, compact = false, columns = 0, wrap = false }: {
        label: string;
        /** `name` is the name of an option for a screen reader when its label is short, such as "Sunday" for "Sun". */
        options: { value: T; label: string; icon?: string; name?: string }[];
        value: T;
        onChange: (value: T) => void;
        compact?: boolean;
        /** Lays the options out in a grid of this many columns instead of one row. */
        columns?: number;
        /** Lets the options take more rows, each option as wide as its label needs. */
        wrap?: boolean;
    } = $props();

    // A radio group keeps one tab stop, also when no option matches.
    const stop = $derived(options.some(option => option.value === value) ? value : options[0]?.value);
</script>

<div class="segmented" class:compact class:wrap class:grid={columns > 0} style:grid-template-columns={columns > 0 ? `repeat(${columns}, 1fr)` : undefined} role="radiogroup" aria-label={label}>
    {#each options as option, index (option.value)}
        <button
            type="button"
            role="radio"
            aria-checked={value === option.value}
            aria-label={option.name}
            tabindex={stop === option.value ? 0 : -1}
            onclick={() => onChange(option.value)}
            onkeydown={(event) => radioKey(event, index, options.length, next => onChange(options[next].value))}
        >
            {#if option.icon}<Icon name={option.icon} size={15} />{/if}{option.label}
        </button>
    {/each}
</div>

<style>
    .segmented {
        display: flex;
        padding: 2px;
        gap: 2px;
        border-radius: 6px;
        background: var(--parchment-2);
    }
    button {
        flex: 1;
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 6px;
        min-height: 32px;
        padding: 0 8px;
        border: 0;
        border-radius: 5px;
        background: transparent;
        color: var(--ink-soft);
        font: 600 13px var(--sans);
        white-space: nowrap;
        cursor: pointer;
    }
    button:hover {
        color: var(--ink);
    }
    button[aria-checked="true"] {
        background: var(--panel);
        color: var(--ink);
        box-shadow: inset 0 0 0 1px var(--line-strong);
    }
    .grid {
        display: grid;
    }
    .wrap {
        flex-wrap: wrap;
    }
    .wrap button {
        flex: 1 1 auto;
    }
    .compact button {
        min-height: 26px;
        padding: 0 12px;
    }
</style>
