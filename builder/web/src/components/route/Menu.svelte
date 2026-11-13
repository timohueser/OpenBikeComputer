<script lang="ts">
    /** A button of a detail page, such as "More" or "Places", and its menu. A click outside the menu or Escape closes it. */
    let { label, items, left = false }: {
        label: string;
        /** `rule` draws a line over the item; `checked` marks the chosen item of a choice; `danger` is an action that
         * removes something. */
        items: { label: string; note?: string; disabled?: boolean; rule?: boolean; checked?: boolean; danger?: boolean; run: () => void }[];
        /** The menu opens to the right of the button, not to its left. */
        left?: boolean;
    } = $props();
    let open = $state(false), root = $state<HTMLElement>();
</script>

<svelte:window onclick={event => { if (open && !root?.contains(event.target as Node)) open = false; }}
    onkeydown={event => { if (open && event.key === 'Escape') open = false; }} />

<div class="menu" bind:this={root}>
    <button type="button" class="button" aria-expanded={open} onclick={() => open = !open}>{label} ▾</button>
    {#if open}
        <menu class:left>
            {#each items as item (item.label)}
                <li class:rule={item.rule}>
                    <button type="button" class:danger={item.danger} disabled={item.disabled} onclick={() => { open = false; item.run(); }}>
                        <span>{#if item.checked !== undefined}<i>{item.checked ? '✓' : ''}</i>{/if}{item.label}</span>
                        {#if item.note}<small>{item.note}</small>{/if}
                    </button>
                </li>
            {/each}
        </menu>
    {/if}
</div>

<style>
    .menu {
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
    menu.left {
        right: auto;
        left: 0;
    }
    .rule {
        margin-top: 4px;
        padding-top: 4px;
        border-top: 1px solid var(--line);
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
    menu button.danger {
        color: var(--coral);
    }
    i {
        display: inline-block;
        width: 18px;
        font-style: normal;
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
