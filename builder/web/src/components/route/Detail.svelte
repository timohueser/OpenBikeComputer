<script lang="ts">
    import { tick, type Snippet } from 'svelte';
    import Icon from '../planner/PlannerIcon.svelte';
    import { ROUTES } from '../../lib/routes';

    /** The frame of the Route page and the Ride page: the title row with rename, the left card and the picture side by
     * side, and `children` under them. Its button styles apply only inside the frame. */
    let { message, name, nameLabel, onrename, failure, tags, buttons, left, picture, children }: {
        /** Shows instead of the page, for example while the record loads. */
        message?: string;
        name: string;
        /** The label of the name field, such as "Route name". */
        nameLabel: string;
        /** Gets a new name that differs from `name`, at most 120 characters. */
        onrename: (name: string) => void;
        /** The error of the last action, under the title. */
        failure?: string;
        tags?: Snippet;
        buttons?: Snippet;
        left?: Snippet;
        picture?: Snippet;
        children?: Snippet;
    } = $props();

    let renaming = $state(false), input = $state<HTMLInputElement>();

    // A new record ends a rename; a page shows its message while the next record loads.
    $effect(() => { void [name, message]; renaming = false; });

    async function startRename() {
        renaming = true;
        await tick();
        input?.select();
    }

    function rename() {
        const next = input?.value.trim().slice(0, 120);
        renaming = false;
        if (next && next !== name) onrename(next);
    }
</script>

<div class="detail">
    {#if message}
        <p class="message">{message}</p>
        <a class="back" href={ROUTES.routes}>‹ Routes</a>
    {:else}
        <header>
            <a class="back" href={ROUTES.routes}>‹ Routes</a>
            <div class="title">
                {#if renaming}
                    <input bind:this={input} aria-label={nameLabel} value={name}
                        onkeydown={event => { if (event.key === 'Enter') rename(); else if (event.key === 'Escape') renaming = false; }}
                        onblur={() => renaming = false} />
                {:else}
                    <h1>{name}</h1>
                    <button type="button" class="icon" aria-label="Rename" onclick={startRename}><Icon name="pencil" size={16} /></button>
                {/if}
                {@render tags?.()}
            </div>
            {#if buttons}<nav class="buttons" aria-label="Actions">{@render buttons()}</nav>{/if}
        </header>
        {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
        <div class="cards">
            {@render left?.()}
            {@render picture?.()}
        </div>
        {@render children?.()}
    {/if}
</div>

<style>
    .detail {
        display: flex;
        flex-direction: column;
        gap: 12px;
        height: 100%;
        padding: 14px 20px 18px;
    }
    .message {
        margin: 32px 0 8px;
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 10px 20px;
    }
    .back {
        color: var(--ink-soft);
        text-decoration: none;
    }
    .back:hover {
        color: var(--ink);
    }
    .title {
        display: flex;
        flex: 1;
        align-items: center;
        gap: 10px;
        min-width: 0;
    }
    h1 {
        overflow: hidden;
        margin: 0;
        font: 700 25px/1.2 var(--sans);
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    /* The name stays where the title was, in the same letters. */
    .title input {
        width: min(480px, 100%);
        margin: -1px 0 -1px -7px;
        padding: 0 6px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 700 25px/1.2 var(--sans);
    }
    .icon {
        display: grid;
        flex: none;
        place-items: center;
        width: 30px;
        height: 30px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
    }
    .icon:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .title :global(.tag) {
        flex: none;
        padding: 2px 8px;
        border-radius: 4px;
        background: var(--parchment-2);
        color: var(--ink-soft);
        font-size: 12px;
    }
    .title :global(.plain) {
        flex: none;
        color: var(--ink-soft);
        font-size: 15px;
    }
    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }
    .detail :global(.button) {
        display: inline-flex;
        align-items: center;
        padding: 6px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        text-decoration: none;
        white-space: nowrap;
    }
    .detail :global(.button:hover) {
        background: var(--parchment-2);
    }
    .detail :global(.primary), .detail :global(.primary:hover) {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    .detail :global(.danger) {
        border-color: var(--coral);
        color: var(--coral);
    }
    .detail :global(.window-buttons) {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
    }
    .failure {
        margin: 0;
        color: var(--coral);
    }
    .cards {
        display: grid;
        flex: 1;
        grid-template-columns: 300px 1fr;
        grid-template-rows: minmax(0, 1fr);
        gap: 14px;
        min-height: 360px;
    }
    @media (max-width: 760px) {
        .detail {
            height: auto;
            padding: 12px 16px;
        }
        header .back {
            flex-basis: 100%;
        }
        h1, .title input {
            font-size: 21px;
        }
        h1 {
            white-space: normal;
        }
        .cards {
            grid-template-columns: 1fr;
            grid-template-rows: auto;
        }
    }
</style>
