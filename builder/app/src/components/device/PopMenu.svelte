<!--
  The ⋯ menu, as every tile and band carries it: a `<details>` element — no popover machinery
  exists in this codebase, and none is needed for a handful of items. Items are buttons the caller
  renders into the pop; close them with `menuPick` (lib/ui/menu.ts), which also handles the
  "Keep on device" sub-`<details>` by closing the whole ancestor chain.
-->
<script lang="ts">
    let {
        label,
        children,
    }: {
        /** The accessible name of the ⋯ trigger — "Route actions". */
        label: string;
        /** The menu items: buttons (and at most one sub-`<details>`), closed via `menuPick`. */
        children: import("svelte").Snippet;
    } = $props();
</script>

<details class="menu">
    <summary class="iconbtn" aria-label={label} onclick={(e) => e.stopPropagation()}>⋯</summary>
    <div class="pop" role="menu">
        {@render children()}
    </div>
</details>

<style>
    .menu {
        position: relative;
        flex: none;
    }

    .menu summary {
        list-style: none;
        cursor: pointer;
        user-select: none;
    }

    .menu summary::-webkit-details-marker {
        display: none;
    }

    .pop {
        position: absolute;
        right: 0;
        top: calc(100% + 4px);
        z-index: 30;
        min-width: 190px;
        display: flex;
        flex-direction: column;
        padding: 4px;
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 9px;
        box-shadow: 0 8px 22px rgba(36, 51, 28, 0.18);
    }

    /* The items are rendered by the caller, so their styling lives here as :global under the
       pop's own scope — one look for every menu without each caller re-declaring it. */
    .pop :global(button) {
        border: 0;
        background: none;
        text-align: left;
        font: inherit;
        font-size: 13px;
        color: var(--ink);
        padding: 6px 10px;
        border-radius: 6px;
        cursor: pointer;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .pop :global(button:hover) {
        background: var(--parchment-2);
    }

    .pop :global(button.danger) {
        color: var(--coral);
    }

    /* The one nested `<details>` — "Keep on device" — expands inline inside the pop. */
    .pop :global(details > summary) {
        list-style: none;
        font-size: 13px;
        color: var(--ink);
        padding: 6px 10px;
        border-radius: 6px;
        cursor: pointer;
        user-select: none;
        white-space: nowrap;
    }

    .pop :global(details > summary::-webkit-details-marker) {
        display: none;
    }

    .pop :global(details > summary:hover) {
        background: var(--parchment-2);
    }

    .pop :global(details[open] > summary) {
        color: var(--ink-soft);
    }

    .pop :global(details button) {
        display: block;
        width: 100%;
        padding-left: 22px;
    }
</style>
