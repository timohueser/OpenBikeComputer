<script lang="ts">
    import type { Snippet } from 'svelte';

    /** A map control that opens a panel to its left: `label` names the button and the panel. */
    let { label, button, children }: { label: string; button: Snippet; children: Snippet } = $props();

    let open = $state(false);
    let root: HTMLDivElement;
    let opener: HTMLButtonElement;

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }

    // A focus that moves to another control closes the panel, so only one panel of the controls is open.
    function leave(event: FocusEvent) {
        if (open && event.relatedTarget && !root.contains(event.relatedTarget as Node)) open = false;
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open) return;
        event.stopPropagation();
        open = false;
        opener.focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="control" bind:this={root} onkeydown={key} onfocusout={leave}>
    <button type="button" class:chosen={open} bind:this={opener} aria-label={label} aria-expanded={open} aria-haspopup="dialog" onclick={() => open = !open}>{@render button()}</button>
    {#if open}
        <div class="panel" role="dialog" aria-label={label} tabindex="-1">{@render children()}</div>
    {/if}
</div>

<style>
    .control { border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); }
    button {
        display: grid; place-items: center; width: 36px; height: 36px; padding: 0;
        border: 0; border-radius: 8px; background: none; color: var(--ink); cursor: pointer;
    }
    button:hover, button.chosen { color: var(--link); }
    /* Positioned against the controls column, so it starts level with the first control and stays inside the map. */
    .panel {
        position: absolute; top: 0; right: 44px;
        width: min(288px, calc(100vw - 80px)); box-sizing: border-box;
        max-height: calc(var(--map-height, 100vh) - 32px); overflow: auto;
        padding: 12px; border-radius: 8px;
        background: var(--panel); box-shadow: var(--planner-shadow); font-size: 13px;
    }
</style>
