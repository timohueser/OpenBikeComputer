<!-- One choice of a slot of the planner search, a row with a dot, and what belongs to the choice under it once it is
     chosen, such as the kinds under a kind. The panel decides what a press on the chosen row does. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import { unfold } from '../../lib/ui/search-bar/motion';

    let { label, icon, path, on, onclick, children }: {
        label: string; icon?: string;
        /** A 24 px stroke path in place of the named icon. */
        path?: string;
        on: boolean; onclick: () => void; children?: Snippet;
    } = $props();
</script>

<div class="choice" class:on>
    <button type="button" aria-pressed={on} {onclick}><span class="dot"></span><span class="icon"><Icon name={icon} {path} /></span><span class="label">{label}</span></button>
    {#if on && children}<div class="under" transition:unfold>{@render children()}</div>{/if}
</div>

<style>
    .choice {
        margin: 0 -8px;
        padding: 0 8px;
        border-radius: 8px;
        transition: background 120ms;
    }
    .choice:hover {
        background: var(--parchment);
    }
    button {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        min-height: 36px;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink);
        font: 13.5px var(--sans);
        text-align: left;
    }
    .dot {
        flex: none;
        width: 16px;
        height: 16px;
        border: 1.5px solid var(--line-strong);
        border-radius: 50%;
        background: var(--panel);
        transition: border-color 120ms, box-shadow 120ms;
    }
    button:hover .dot {
        border-color: var(--ink-soft);
    }
    .on .dot {
        border-color: var(--ink);
        box-shadow: inset 0 0 0 4px var(--ink);
    }
    .icon {
        display: flex;
        color: var(--ink-soft);
    }
    .on .icon {
        color: var(--ink);
    }
    .on .label {
        font-weight: 600;
    }
    .under {
        padding: 0 0 10px 26px;
    }
    /* A row of small choices under the chosen row, such as the kinds under a kind or the days of the week. */
    .under :global(.pills) {
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
    }
    .under :global(.pills button) {
        width: auto;
        min-height: 28px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 14px;
        background: var(--panel);
        color: var(--ink-soft);
        font: 12.5px var(--sans);
        transition: background 120ms, border-color 120ms, color 120ms;
    }
    .under :global(.pills button:hover) {
        border-color: var(--ink-faint);
        color: var(--ink);
    }
    .under :global(.pills button[aria-pressed="true"]) {
        border-color: var(--ink);
        background: var(--ink);
        color: var(--panel);
        font-weight: 600;
    }
</style>
