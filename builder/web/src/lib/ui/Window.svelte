<script lang="ts">
    import { onMount, type Snippet } from 'svelte';
    import Icon from '../../components/planner/PlannerIcon.svelte';

    let { title, onClose, children }: { title: string; onClose: () => void; children: Snippet } = $props();
    let dialog: HTMLDialogElement;
    const heading = $props.id();
    onMount(() => dialog.showModal());
</script>

<!-- Esc closes a modal dialog by itself, and each way of closing ends in `onclose`. -->
<dialog bind:this={dialog} aria-labelledby={heading} onclose={onClose}>
    <header>
        <h2 id={heading}>{title}</h2>
        <button type="button" class="close" aria-label="Close" onclick={() => dialog.close()}><Icon name="close" /></button>
    </header>
    {@render children()}
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
    dialog::backdrop { background: rgb(28 27 20 / 0.55); }
    :global([data-theme="dark"]) dialog::backdrop { background: rgb(0 0 0 / 0.55); }
    header {
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
