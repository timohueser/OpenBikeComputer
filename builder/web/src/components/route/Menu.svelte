<script lang="ts">
    /** A button of a detail page, such as "More" or "Places", and its menu. The menu is in the top layer, so no card cuts
     * it: it opens under its button and moves up or to the side where it would leave the window. A click outside the
     * menu, Escape or a new size of the window closes it. */
    let { label, items, left = false }: {
        label: string;
        /** `rule` draws a line over the item; `checked` marks the chosen item of a choice; `danger` is an action that
         * removes something. */
        items: { label: string; note?: string; disabled?: boolean; rule?: boolean; checked?: boolean; danger?: boolean; run: () => void }[];
        /** The menu opens to the right of the button, not to its left. */
        left?: boolean;
    } = $props();
    /** The menu keeps EDGE_PX from the edges of the window and GAP_PX from its button. */
    const EDGE_PX = 8, GAP_PX = 4;
    let open = $state(false), root = $state<HTMLElement>(), button = $state<HTMLButtonElement>();

    function place(menu: HTMLElement) {
        menu.showPopover();
        const from = button!.getBoundingClientRect(), box = menu.getBoundingClientRect();
        const inside = (wanted: number, size: number, room: number) => Math.max(EDGE_PX, Math.min(wanted, room - EDGE_PX - size));
        menu.style.left = `${inside(left ? from.left : from.right - box.width, box.width, innerWidth)}px`;
        menu.style.top = `${inside(from.bottom + GAP_PX, box.height, innerHeight)}px`;
    }

    // In the capture phase, so that Escape closes the menu before anything under it, such as a selected part.
    function escape(event: KeyboardEvent) {
        if (!open || event.key !== 'Escape') return;
        event.preventDefault();
        open = false;
        button?.focus();
    }
</script>

<svelte:window onclick={event => { if (open && !root?.contains(event.target as Node)) open = false; }}
    onkeydowncapture={escape} onresize={() => open = false} />

<div class="menu" bind:this={root}>
    <button type="button" class="button" bind:this={button} aria-expanded={open} onclick={() => open = !open}>{label} ▾</button>
    {#if open}
        <menu popover="manual" use:place>
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
    /* `place` sets the left and the top edge. In a window lower than the menu, the menu scrolls. */
    menu {
        position: fixed;
        inset: auto;
        min-width: 140px;
        max-height: calc(100dvh - 16px);
        margin: 0;
        padding: 4px;
        overflow-y: auto;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
        box-shadow: 0 8px 24px rgb(28 27 20 / 0.18);
        list-style: none;
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
</style>
