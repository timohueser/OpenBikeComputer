<script lang="ts">
    import { onMount, type Snippet } from 'svelte';
    import Icon from '../../components/planner/PlannerIcon.svelte';

    /**
     * `children` gets `close`, which closes the dialog as Esc and the close button do: the focus goes back.
     * A `large` window is a page over the page: a click on the dimmed page closes it, and on a phone it fills the screen.
     */
    let { title, large = false, onClose, children }: {
        title: string; large?: boolean; onClose: () => void; children: Snippet<[() => void]>;
    } = $props();
    let dialog: HTMLDialogElement;
    const heading = $props.id();
    onMount(() => dialog.showModal());
    const close = () => dialog.close();
    // A click on the backdrop has the dialog as its target, as a click on its padding does.
    function outside(event: MouseEvent) {
        if (event.target !== dialog) return;
        const box = dialog.getBoundingClientRect();
        if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) close();
    }
</script>

<!-- Esc closes a modal dialog by itself, and each way of closing ends in `onclose`. -->
<dialog bind:this={dialog} class:large aria-labelledby={heading} onclose={onClose} onclick={large ? outside : undefined}>
    <header>
        <h2 id={heading}>{title}</h2>
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
        width: min(1080px, calc(100vw - 48px));
        max-height: calc(100dvh - 48px);
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
    dialog::backdrop { background: rgb(28 27 20 / 0.55); }
    :global([data-theme="dark"]) dialog::backdrop { background: rgb(0 0 0 / 0.55); }
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
        flex: 1;
        margin: 0;
        font: 600 17px/1.3 var(--sans);
    }
    .close {
        display: grid;
        place-items: center;
        width: 32px;
        height: 32px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
    }
    .close:hover { background: var(--parchment-2); color: var(--ink); }
</style>
