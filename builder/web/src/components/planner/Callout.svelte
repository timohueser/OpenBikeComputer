<script lang="ts">
    import { onMount, type Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';

    /** The frame of a map popup: a close button, Escape, and the focus, which it takes on mount unless `takeFocus` is
     * false and gives back to the opener on close. It never takes the focus from a control that the rider chose before
     * its first frame. Its style gives the parts of each popup one look: a `header` with the title and a `.meta` line,
     * `.buttons`, the `.rows` of a menu or a list, a `.rule` over a new group, and a `footer` of main actions. */
    let { label, onClose, takeFocus = true, children }: { label: string; onClose: () => void; takeFocus?: boolean; children: Snippet } = $props();

    let root: HTMLDivElement;
    let opener: HTMLElement | null = null;

    onMount(() => {
        opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        // The map moves this content into its popup after mounting.
        // MapLibre can focus the close button as it adds the popup.
        const frame = requestAnimationFrame(() => {
            if (!takeFocus) return;
            const at = document.activeElement;
            if (at === opener || at === document.body || root.contains(at)) root.focus({ preventScroll: true });
        });
        return () => cancelAnimationFrame(frame);
    });

    function close() {
        onClose();
        if (opener?.isConnected) opener.focus({ preventScroll: true });
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || (event.target as HTMLElement).closest('input, select')) return;
        event.stopPropagation();
        close();
    }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="callout" bind:this={root} tabindex="-1" role="dialog" aria-label={label} onkeydown={key}>
    <button type="button" class="close" onclick={close} aria-label="Close"><Icon name="close" size={15} /></button>
    {@render children()}
</div>

<style>
    /* The map sets the free room; the content scrolls as a last resort. The parts stand 12 px apart. */
    .callout {
        position: relative;
        display: flex;
        flex-direction: column;
        gap: 12px;
        width: 340px;
        max-width: var(--callout-width);
        max-height: var(--callout-room);
        overflow-y: auto;
        padding: 16px 16px 0;
        color: var(--ink);
        font: 400 13px/1.45 var(--sans);
        outline: none;
    }
    .callout > :global(*) {
        flex: none;
    }
    /* The last part holds the room under it: the footer fills that room, and a negative margin would move it as it sticks. */
    .callout > :global(:last-child) {
        margin-bottom: 16px;
    }
    .close {
        position: absolute;
        top: 10px;
        right: 10px;
        z-index: 1;
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: inherit;
        cursor: pointer;
    }
    .close:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }

    /* The title keeps clear of the close button. */
    .callout :global(h2) {
        margin: 0 28px 0 0;
        font: 600 16px/1.3 var(--sans);
        overflow-wrap: anywhere;
    }
    .callout :global(p) {
        margin: 0;
    }
    .callout :global(.meta) {
        margin-top: 2px;
        color: var(--ink-soft);
    }
    .callout :global(a) {
        color: var(--ink);
        text-underline-offset: 3px;
    }
    .callout :global(a:hover) {
        color: var(--link);
    }

    .callout :global(.buttons) {
        display: flex;
        gap: 8px;
    }
    .callout :global(.planner-action) {
        flex: 1;
        gap: 7px;
        min-height: 36px;
    }

    /* A row is an action of a menu or an entry of a list: a symbol, a name, and a note at the right. Its fill reaches
       8 px past the text on each side, so the symbols stand in line with the title. */
    .callout :global(.rows) {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        margin: -4px -8px;
        padding: 0;
        list-style: none;
    }
    .callout :global(.rows:last-child) {
        margin-bottom: 8px;
    }
    .callout :global(.row) {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        min-height: 34px;
        padding: 0 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
        font: 600 13px var(--sans);
        text-align: left;
        cursor: pointer;
    }
    .callout :global(.row:hover) {
        background: var(--parchment-2);
    }
    .callout :global(.row:focus-visible) {
        outline-offset: -2px;
    }
    .callout :global(.row svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .callout :global(.row span),
    .callout :global(.row small) {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    /* In a narrow popup the note gives way to the name. */
    .callout :global(.row small) {
        flex-shrink: 100;
        margin-left: auto;
        color: var(--ink-soft);
        font: 400 13px var(--sans);
    }
    .callout :global(.row.danger),
    .callout :global(.row.danger svg) {
        color: var(--coral);
    }

    /* A line over a new group, from edge to edge. */
    .callout :global(.rule),
    .callout :global(.hours),
    .callout :global(footer) {
        margin-inline: -16px;
        padding: 12px 16px 0;
        border-top: 1px solid var(--line);
    }
    .callout :global(.rows.rule) {
        margin-top: 0;
        padding: 8px 8px 0;
    }
    /* The main actions stay in view at the bottom while a long popup scrolls. */
    .callout :global(footer) {
        position: sticky;
        bottom: 0;
        display: grid;
        gap: 8px;
        padding-bottom: 12px;
        background: var(--panel);
    }
    .callout :global(footer:last-child) {
        margin-bottom: 0;
        padding-bottom: 16px;
    }
    .callout :global(footer + .rows.rule) {
        margin-top: -12px;
    }
    /* Under the title alone, the actions need no line. */
    .callout :global(header + footer) {
        padding-top: 0;
        border-top: 0;
    }
</style>
