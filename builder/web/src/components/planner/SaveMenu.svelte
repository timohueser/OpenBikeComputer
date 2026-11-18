<script lang="ts">
    // PROTOTYPE (prototype/save-bar): the Save button and its menu, with fake state.
    import { tick } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import { proto } from './save-prototype.svelte';

    let { canSave }: { canSave: boolean } = $props();

    let open = $state(false);
    let root: HTMLDivElement;
    let menu = $state<HTMLDivElement>();

    const saved = $derived(proto.stored && !proto.changed);

    function save() {
        if (!proto.stored) proto.window = 'name';
        else proto.changed = false;
    }

    async function toggle() {
        open = !open;
        if (!open) return;
        await tick();
        menu?.querySelector<HTMLElement>('button')?.focus();
    }

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }
</script>

<svelte:window onpointerdown={outside} />

<div class="save" class:alt-b={proto.alt === 'B'} bind:this={root}>
    {#if proto.alt === 'A'}
        <div class="split" class:saved>
            <button type="button" class="main" disabled={saved || !canSave} onclick={save}>
                {#if saved}<Icon name="check" size={15} />Saved{:else}Save{/if}
            </button>
            <button type="button" class="arrow" aria-label="More save options" title="More save options" aria-expanded={open} aria-haspopup="true" onclick={toggle}>
                <Icon name="down" size={14} />
            </button>
        </div>
    {:else if proto.alt === 'C'}
        {#if proto.stored && proto.changed}<button type="button" class="quiet-action">Discard changes</button>{/if}
        <button type="button" class="quiet-action" disabled={!canSave}>Save a copy…</button>
        <button type="button" class="plain" class:saved disabled={saved || !canSave} onclick={save}>
            {#if saved}<Icon name="check" size={15} />Saved{:else}Save{/if}
        </button>
    {:else}
        <button type="button" class="plain" class:saved disabled={saved || !canSave} onclick={save}>
            {#if saved}<Icon name="check" size={15} />Saved{:else}Save{/if}
        </button>
        <button type="button" class="more" aria-label="More save options" title="More save options" aria-expanded={open} aria-haspopup="true" onclick={toggle}>
            <Icon path="M5 12h.01M12 12h.01M19 12h.01" size={20} />
        </button>
    {/if}
    {#if open}
        <div class="menu" bind:this={menu}>
            <button type="button" class="item" disabled={!canSave}>Save a copy…</button>
            <button type="button" class="item" disabled={!proto.changed || !proto.stored}>Discard changes</button>
            <div class="rule"></div>
            <button type="button" class="item">Compare…</button>
        </div>
    {/if}
</div>

<style>
    .save {
        position: relative;
        display: flex;
        align-items: center;
        gap: 4px;
        margin-left: 8px;
    }
    button {
        font: inherit;
        color: inherit;
        cursor: pointer;
    }
    .split {
        display: flex;
        height: 32px;
        border-radius: 6px;
        background: var(--amber);
        color: var(--on-amber);
    }
    .split button {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        border: 0;
        background: none;
        color: inherit;
        font: 600 13px var(--sans);
    }
    .split .main { padding: 0 14px; border-radius: 6px 0 0 6px; }
    .split .arrow {
        padding: 0 8px;
        border-left: 1px solid color-mix(in srgb, var(--on-amber) 22%, transparent);
        border-radius: 0 6px 6px 0;
    }
    .split button:hover:not(:disabled) { background: color-mix(in srgb, var(--ink) 8%, transparent); }
    .split.saved {
        background: none;
        color: var(--ink-soft);
        box-shadow: inset 0 0 0 1px var(--line-strong);
    }
    .split.saved .main { cursor: default; padding-left: 10px; }
    .split.saved .arrow { border-left-color: var(--line-strong); color: var(--ink); }
    .split.saved .arrow:hover { background: var(--parchment-2); }

    .plain {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        height: 32px;
        padding: 0 16px;
        border: 0;
        border-radius: 6px;
        background: var(--amber);
        color: var(--on-amber);
        font: 600 13px var(--sans);
    }
    .plain:hover:not(:disabled) { filter: brightness(.95); }
    .plain.saved {
        padding: 0 8px;
        background: none;
        color: var(--ink-soft);
        cursor: default;
    }
    .more {
        display: grid;
        place-items: center;
        width: 30px;
        height: 30px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
    }
    .more:hover, .more[aria-expanded="true"] { background: var(--parchment-2); }
    .more :global(svg) { stroke-width: 3; }

    .rule { margin: 4px 0; border-top: 1px solid var(--line); }
    .split.saved .main, .plain.saved { opacity: 1; }
    .quiet-action {
        display: inline-flex;
        align-items: center;
        height: 32px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink-soft);
        font: 600 13px var(--sans);
    }
    .quiet-action:hover { background: var(--parchment-2); color: var(--ink); }
    .menu {
        position: absolute;
        top: 40px;
        right: 0;
        z-index: 30;
        width: min(220px, calc(100vw - 32px));
        padding: 6px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    .item {
        display: block;
        width: 100%;
        padding: 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        text-align: left;
        font-size: 13px;
        font-weight: 600;
    }
    .item:hover:not(:disabled) { background: var(--parchment-2); }
    .item:disabled { color: var(--ink-faint); cursor: default; }
    .group {
        margin: 4px 0;
        padding: 4px 0;
        border-block: 1px solid var(--line);
    }
    .head {
        margin: 0;
        padding: 6px 8px 2px;
        font-size: 11px;
        font-weight: 600;
        color: var(--ink-soft);
    }
    .empty {
        margin: 0;
        padding: 4px 8px 8px;
        font-size: 13px;
        line-height: 1.4;
        color: var(--ink-soft);
    }
    ul { margin: 0; padding: 0; list-style: none; }
    li {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 4px 8px;
        border-radius: 6px;
    }
    li:hover { background: var(--parchment-2); }
    .what {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        gap: 12px;
        padding: 2px 0;
        border: 0;
        background: none;
        text-align: left;
        font-variant-numeric: tabular-nums;
    }
    .what strong { font: 600 13px var(--sans); white-space: nowrap; }
    .what small { font-size: 12px; color: var(--ink-soft); white-space: nowrap; }
    .delete {
        display: grid;
        place-items: center;
        width: 24px;
        height: 24px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-faint);
    }
    .delete:hover { color: var(--ink); }
</style>
