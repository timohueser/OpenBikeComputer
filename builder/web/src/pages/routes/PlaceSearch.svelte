<!-- A field that finds places near the map view and offers them to choose. -->
<script lang="ts">
    import { onDestroy } from 'svelte';
    import { findPlaces, type Box, type FoundPlace } from './filter';

    let { view, label, placeholder, autofocus = false, float = false, onchoose }: {
        view?: Box;
        label: string;
        placeholder: string;
        autofocus?: boolean;
        /** The places open over what is below the field, and close on a choice, on a click outside or on Escape. */
        float?: boolean;
        onchoose: (place: FoundPlace) => void;
    } = $props();

    const WAIT_MS = 300;
    let query = $state(''), found = $state.raw<FoundPlace[] | null>(null), failure = $state(''), field = $state<HTMLElement>();
    let timer: ReturnType<typeof setTimeout> | undefined, controller: AbortController | undefined;

    function search() {
        clearTimeout(timer);
        controller?.abort();
        const text = query.trim();
        [found, failure] = [null, ''];
        if (!text) return;
        timer = setTimeout(() => {
            const current = controller = new AbortController();
            findPlaces(text, view ?? [-180, -85, 180, 85], current.signal).then(
                places => { if (!current.signal.aborted) found = places; },
                reason => { if (!current.signal.aborted) failure = reason instanceof Error ? reason.message : String(reason); });
        }, WAIT_MS);
    }

    function choose(place: FoundPlace) {
        if (float) [query, found] = [place.name, null];
        onchoose(place);
    }
    onDestroy(() => { clearTimeout(timer); controller?.abort(); });
</script>

<!-- Escape in an open window closes only the window. -->
<svelte:window onclick={event => { if (float && found && !field?.contains(event.target as Node)) found = null; }}
    onkeydown={event => { if (float && event.key === 'Escape' && !document.querySelector('dialog[open]')) found = null; }} />

<div class="place-search" class:float bind:this={field}>
    <!-- svelte-ignore a11y_autofocus -->
    <input type="search" aria-label={label} {placeholder} {autofocus} bind:value={query} oninput={search} />
    {#if failure || found}
        <div class="found">
            {#each found ?? [] as place (place.box.join() + place.name)}
                <button type="button" onclick={() => choose(place)}>
                    {place.name}<small>{[place.kind.replaceAll('_', ' '), place.region].filter(Boolean).join(' · ')}</small>
                </button>
            {:else}
                <p class="note">{failure || 'No place found.'}</p>
            {/each}
        </div>
    {/if}
</div>

<style>
    .place-search {
        display: flex;
        flex-direction: column;
    }
    input {
        min-width: 220px;
        margin: 2px 2px 4px;
        padding: 6px 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
    }
    .found {
        display: flex;
        flex-direction: column;
    }
    button {
        display: flex;
        flex-direction: column;
        padding: 6px 10px;
        border: 0;
        border-radius: 5px;
        background: none;
        color: var(--ink);
        text-align: left;
        white-space: nowrap;
    }
    button:hover {
        background: var(--parchment-2);
    }
    small, .note {
        color: var(--ink-soft);
    }
    .note {
        margin: 4px 8px 6px;
    }
    .float {
        position: relative;
    }
    .float input {
        width: 100%;
        min-width: 0;
        margin: 0;
        padding: 8px 10px;
        border-radius: 8px;
    }
    .float button {
        white-space: normal;
    }
    /* The places open over the list, so the list does not move. */
    .float .found {
        position: absolute;
        z-index: 6;
        top: calc(100% + 4px);
        right: 0;
        left: 0;
        padding: 4px;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        box-shadow: 0 8px 24px rgb(28 27 20 / 0.18);
    }
</style>
