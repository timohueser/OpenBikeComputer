<script lang="ts">
    /** The credits of an image of the map data, behind a button in a lower corner of the positioned parent. */
    let { credits, side = 'right' }: { credits: string[]; side?: 'left' | 'right' } = $props();
    let open = $state(false);
    let box = $state<HTMLElement>();
    const id = $props.id();
</script>

<svelte:window onclick={event => { if (open && !box?.contains(event.target as Node)) open = false; }} />

{#if credits.length}
    <!-- Touch has no hover, so a tap toggles the credits. -->
    <div class="credits {side}" class:open bind:this={box}>
        <small {id} role="tooltip">{credits.join(' · ')}</small>
        <button type="button" aria-label="Credits" aria-describedby={id}
            onclick={event => { if ('pointerType' in event && event.pointerType === 'touch') open = !open; }}>
            <svg viewBox="0 0 20 20" aria-hidden="true"><path fill="currentColor" fill-rule="evenodd" d="M4 10a6 6 0 1 0 12 0 6 6 0 1 0-12 0m5-3a1 1 0 1 0 2 0 1 1 0 1 0-2 0m0 3a1 1 0 1 1 2 0v3a1 1 0 1 1-2 0" /></svg>
        </button>
    </div>
{/if}

<style>
    .credits {
        position: absolute;
        z-index: 2;
        bottom: max(10px, env(safe-area-inset-bottom));
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    .right {
        right: max(10px, env(safe-area-inset-right));
        align-items: flex-end;
    }
    .left {
        left: max(10px, env(safe-area-inset-left));
        align-items: flex-start;
    }
    small {
        display: none;
        max-width: min(320px, 70vw);
        padding: 3px 8px;
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink-soft);
        font: 11px var(--sans);
    }
    button {
        display: grid;
        place-items: center;
        width: 24px;
        height: 24px;
        padding: 0;
        border: 0;
        border-radius: 12px;
        background: var(--panel);
        color: var(--ink);
    }
    /* A tap leaves :hover and :focus on the button: only a real hover and keyboard focus show the credits. */
    .credits:is(.open, :has(:focus-visible)) small {
        display: block;
    }
    @media (hover: hover) {
        .credits:hover small {
            display: block;
        }
    }
    svg {
        width: 24px;
        height: 24px;
    }
</style>
