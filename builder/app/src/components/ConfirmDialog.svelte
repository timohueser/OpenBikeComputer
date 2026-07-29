<!--
  The app's own confirmation, mounted once at the root — see `lib/ui/confirm.svelte.ts` for why the
  browser's is not usable in the desktop app.

  Deliberately **not** a `<dialog>` element. That would come with focus trapping and Escape for
  free, and it is also the one thing this component must not get wrong: `<dialog>.showModal()` is
  Safari 15.4 / WebKitGTK 2.36 and newer, and the bug being fixed here is precisely a modal that
  silently never appears on an older webview. A plain overlay renders everywhere a `div` does.
-->
<script lang="ts">
    import { confirmQueue } from "../lib/ui/confirm.svelte";

    const pending = $derived(confirmQueue.pending);
    let confirmButton = $state<HTMLButtonElement>();

    // Focus the affirmative button when a question opens, so the keyboard path is Enter / Escape
    // and a screen reader is moved into the dialog rather than left where the click happened.
    $effect(() => {
        if (pending) confirmButton?.focus();
    });

    function onKeydown(event: KeyboardEvent) {
        if (pending && event.key === "Escape") {
            event.preventDefault();
            pending.answer(false);
        }
    }
</script>

<svelte:window on:keydown={onKeydown} />

{#if pending}
    <!-- The backdrop declines rather than doing nothing: a click outside a modal means "not this". -->
    <div
        class="backdrop"
        role="presentation"
        onclick={(e) => e.target === e.currentTarget && pending.answer(false)}
    >
        <div class="sheet card" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title">
            <h3 id="confirm-title">{pending.title}</h3>
            {#if pending.body}
                <p class="small muted">{pending.body}</p>
            {/if}
            <div class="actions">
                <button type="button" class="btn ghost" onclick={() => pending.answer(false)}>
                    Cancel
                </button>
                {#if pending.extra}
                    <!-- The second affirmative, between Cancel and the primary — reachable by
                         Tab from the focused primary, never focused by default. -->
                    <button
                        type="button"
                        class="btn ghost"
                        class:destructive-ghost={pending.extra.destructive}
                        onclick={() => pending.answer("extra")}
                    >
                        {pending.extra.label}
                    </button>
                {/if}
                <button
                    type="button"
                    class="btn primary"
                    class:destructive={pending.destructive}
                    bind:this={confirmButton}
                    onclick={() => pending.answer(true)}
                >
                    {pending.confirmLabel ?? "Confirm"}
                </button>
            </div>
        </div>
    </div>
{/if}

<style>
    .backdrop {
        position: fixed;
        inset: 0;
        z-index: 2000;
        background: rgba(32, 48, 29, 0.38);
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 20px;
    }

    .sheet {
        width: min(440px, 100%);
        display: flex;
        flex-direction: column;
        gap: 10px;
        box-shadow: 0 18px 44px rgba(32, 48, 29, 0.28);
    }

    h3 {
        font-size: 16.5px;
        margin: 0;
    }

    p {
        margin: 0;
        line-height: 1.45;
        white-space: pre-line;
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        margin-top: 4px;
    }

    .destructive {
        background: var(--coral);
        border-color: var(--coral);
    }

    /* A destructive-but-not-primary choice: coral text on the quiet ghost chrome, so the filled
       coral primary stays the loudest thing in the row. */
    .btn.destructive-ghost {
        color: var(--coral);
        border-color: var(--coral);
    }
</style>
