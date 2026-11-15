<script lang="ts">
    import { onMount, type Snippet } from 'svelte';
    import Icon from '../../components/planner/PlannerIcon.svelte';

    /**
     * `children` gets `close`, which closes the dialog as Esc and the close button do: the focus goes back.
     * A `large` window is a page over the page: a click on the dimmed page closes it, and on a phone it fills the screen.
     * A `wide` window is a large window for a content that needs the width, such as a line over a distance.
     * `beside` stands after the title, such as the way back from a closer view of the content.
     */
    let { title, large = false, wide = false, onClose, children, beside }: {
        title: string; large?: boolean; wide?: boolean; onClose: () => void; children: Snippet<[() => void]>; beside?: Snippet;
    } = $props();
    const page = $derived(large || wide);
    let dialog: HTMLDialogElement;
    const heading = $props.id();
    onMount(() => {
        const before = document.activeElement;
        dialog.showModal();
        // A window that goes while open, for example at Back, gets no `close`, so the focus does not go back by itself.
        return () => {
            if (!dialog.open) return;
            dialog.close();
            if (before instanceof HTMLElement) before.focus();
        };
    });
    const close = () => dialog.close();
    // A click on the backdrop has the dialog as its target, as a click on its padding does. A press in the window
    // that ends on the backdrop, as a text selection does, is no click on the backdrop.
    const outside = (event: MouseEvent) => {
        if (event.target !== dialog) return false;
        const box = dialog.getBoundingClientRect();
        return event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom;
    };
    let pressedOutside = false;
</script>

<!-- Esc closes a modal dialog by itself, and each way of closing ends in `onclose`. -->
<dialog bind:this={dialog} class:large={page} class:wide aria-labelledby={heading} onclose={onClose} onpointerdown={page ? event => pressedOutside = outside(event) : undefined}
    onclick={page ? event => { if (pressedOutside && outside(event)) close(); } : undefined}>
    <header>
        <h2 id={heading}>{title}</h2>
        {@render beside?.()}
        <button type="button" class="close" aria-label="Close" onclick={close}><Icon name="close" /></button>
    </header>
    {@render children(close)}
</dialog>

<style>
    dialog {
        width: min(560px, calc(100vw - 32px));
        max-height: calc(100dvh - 64px);
        padding: 0 20px 20px;
        border: 0;
        border-radius: 14px;
        background: var(--panel);
        color: var(--ink);
        font-family: var(--sans);
        box-shadow: 0 18px 48px rgb(28 27 20 / 0.35);
    }
    .large {
        overscroll-behavior: contain;
        width: min(640px, calc(100vw - 48px));
        max-height: calc(100dvh - 48px);
    }
    .wide {
        width: min(1000px, calc(100vw - 48px));
    }
    @media (max-width: 760px) {
        .large {
            width: 100vw;
            max-width: none;
            height: 100dvh;
            max-height: none;
            margin: 0;
            border-radius: 0;
        }
    }
    dialog::backdrop { background: rgb(28 27 20 / 0.7); }
    :global([data-theme="dark"]) dialog::backdrop { background: rgb(0 0 0 / 0.55); }
    /* The text starts under the title, and the row of buttons ends the window: the same in each window. */
    dialog > :global(p) { margin: 0; }
    dialog :global(.window-buttons) {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        margin-top: 20px;
    }
    /* The header stays at the top while the window scrolls. */
    header {
        position: sticky;
        top: 0;
        z-index: 1;
        background: var(--panel);
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 16px 0 12px;
    }
    h2 {
        margin: 0;
        font: 600 17px/1.3 var(--sans);
    }
    .close {
        display: grid;
        place-items: center;
        width: 32px;
        height: 32px;
        margin-left: auto;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
    }
    .close:hover { background: var(--parchment-2); color: var(--ink); }
</style>
