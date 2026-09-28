<script lang="ts" generics="T extends string">
    import Icon from './PlannerIcon.svelte';

    let { label, options, value, onChange, compact = false, columns = 0 }: {
        label: string;
        options: { value: T; label: string; icon?: string }[];
        value: T;
        onChange: (value: T) => void;
        compact?: boolean;
        /** Lays the options out in a grid of this many columns instead of one row. */
        columns?: number;
    } = $props();

    // A radio group keeps one tab stop, also when no option matches.
    const stop = $derived(options.some(option => option.value === value) ? value : options[0]?.value);

    function key(event: KeyboardEvent, index: number) {
        const step = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 0;
        if (!step) return;
        event.preventDefault();
        const next = (index + step + options.length) % options.length;
        onChange(options[next].value);
        ((event.currentTarget as HTMLElement).parentElement!.children[next] as HTMLElement).focus();
    }
</script>

<div class="segmented" class:compact class:grid={columns > 0} style:grid-template-columns={columns > 0 ? `repeat(${columns}, 1fr)` : undefined} role="radiogroup" aria-label={label}>
    {#each options as option, index (option.value)}
        <button
            type="button"
            role="radio"
            aria-checked={value === option.value}
            tabindex={stop === option.value ? 0 : -1}
            onclick={() => onChange(option.value)}
            onkeydown={(event) => key(event, index)}
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
    .compact button {
        min-height: 26px;
        padding: 0 12px;
    }
</style>
