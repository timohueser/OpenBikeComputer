<script lang="ts">
    /** The Export button of a detail page and its menu. A click outside the menu closes it. */
    let { items }: { items: { label: string; note?: string; disabled?: boolean; run: () => void }[] } = $props();
    let open = $state(false);
</script>

<svelte:window onclick={event => { if (open && !(event.target as Element).closest('.export')) open = false; }} />

<div class="export">
    <button type="button" class="button" aria-expanded={open} onclick={() => open = !open}>Export ▾</button>
    {#if open}
        <menu>
            {#each items as item (item.label)}
                <li>
                    <button type="button" disabled={item.disabled} onclick={() => { open = false; item.run(); }}>
                        {item.label}{#if item.note}<small>{item.note}</small>{/if}
                    </button>
                </li>
            {/each}
        </menu>
    {/if}
</div>

<style>
    .export {
        position: relative;
    }
    menu {
        position: absolute;
        z-index: 5;
        top: calc(100% + 4px);
        right: 0;
        min-width: 140px;
        margin: 0;
        padding: 4px;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        box-shadow: 0 8px 24px rgb(28 27 20 / 0.18);
        list-style: none;
    }
    menu button {
        display: flex;
        flex-direction: column;
        width: 100%;
        padding: 7px 10px;
        border: 0;
        border-radius: 5px;
        background: none;
        color: var(--ink);
        text-align: left;
        white-space: nowrap;
    }
    menu button:not(:disabled):hover {
        background: var(--parchment-2);
    }
    small {
        color: var(--ink-soft);
        font-size: 12px;
    }
    @media (max-width: 760px) {
        menu {
            right: auto;
            left: 0;
        }
    }
</style>
