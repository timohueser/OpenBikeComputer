<script lang="ts">
    import { onMount, type Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';

    /** The frame of a map popup: a close button, Escape, and the focus, which it takes on mount unless `takeFocus` is
     * false and gives back to the opener on close. It never takes the focus from a control that the rider chose before
     * its first frame. */
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
    /* The map sets the free room; the content scrolls as a last resort. */
    .callout {
        position: relative;
        width: 340px;
        max-width: var(--callout-width);
        max-height: var(--callout-room);
        overflow-y: auto;
        padding: 16px;
        color: var(--ink);
        font: 400 13px var(--sans);
        outline: none;
    }
    /* Nothing follows the opening hours at the end, so they have no rule and no room under them. */
    .callout > :global(.hours:last-child) {
        margin-bottom: 0;
        padding-bottom: 0;
        border-bottom: 0;
    }
    .close {
        position: absolute;
        top: 10px;
        right: 10px;
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
</style>
