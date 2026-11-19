<script lang="ts" module>
    /** `save`: the first save of a route that is not stored. `copy`: "Save a copy…". `leave`: the question before another
     * route replaces a route with changes. `rename`: a new name for the stored route. */
    export type SaveMode = 'save' | 'copy' | 'leave' | 'rename';
    /** A name to save or rename with, false for "Discard", null for "Cancel". A stored route answers "Save" with ''. */
    export type SaveAnswer = string | false | null;
</script>

<script lang="ts">
    import Window from '../../lib/ui/Window.svelte';
    import { planTitle, type Trip } from '../../lib/planner/editor';
    import { offeredName } from '../../lib/planner/point-names';

    let { mode, name, stored, trip, onAnswer }: {
        mode: SaveMode;
        /** The stored name of the route, or '' when it has none. */
        name: string;
        stored: boolean;
        trip: Trip;
        onAnswer: (answer: SaveAnswer) => void;
    } = $props();

    const title = $derived(name || planTitle(trip));
    const asks = $derived(mode !== 'leave' || !stored);
    const heading = $derived({ save: 'Save route', copy: 'Save a copy', rename: 'Rename route', leave: stored ? `Save changes to ‘${title}’?` : 'Save this route?' }[mode]);
    const action = $derived({ save: 'Save', copy: 'Save copy', rename: 'Rename', leave: 'Save' }[mode]);
    // svelte-ignore state_referenced_locally
    let value = $state(mode === 'copy' ? `${title} (copy)` : mode === 'rename' ? title : offeredName(trip));
    let answer: SaveAnswer = null;

</script>

<!-- The window opens with the focus on the autofocus element: the name, or "Save" of the question. -->
<Window title={heading} onClose={() => onAnswer(answer)}>
    {#snippet children(close)}
        <form onsubmit={event => { event.preventDefault(); answer = asks ? value.trim() : ''; close(); }}>
            {#if asks}
                <label class="field"><span>Name</span><!-- svelte-ignore a11y_autofocus -->
                    <input bind:value maxlength="120" placeholder="Name of the route" autocomplete="off" autofocus onfocus={event => event.currentTarget.select()} /></label>
            {/if}
            <div class="window-buttons">
                <button type="button" class="planner-action" onclick={close}>Cancel</button>
                {#if mode === 'leave'}<button type="button" class="planner-action" onclick={() => { answer = false; close(); }}>Discard</button>{/if}
                <!-- svelte-ignore a11y_autofocus -->
                <button type="submit" class="planner-action primary" disabled={asks && !value.trim()} autofocus={!asks}>{action}</button>
            </div>
        </form>
    {/snippet}
</Window>

<style>
    .field {
        display: flex;
        flex-direction: column;
        gap: 6px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    input {
        height: 38px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 15px var(--sans);
    }
    input::placeholder {
        color: var(--ink-faint);
        font-weight: 400;
    }
</style>
