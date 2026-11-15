<script lang="ts">
    import { untrack } from 'svelte';
    import Window from '../../lib/ui/Window.svelte';
    import type { Library } from '../../lib/library/store';
    import Icon from '../planner/PlannerIcon.svelte';

    /** The window of the photos of a ride: one photo large, with the list of all photos beside it, so a rider with many
     * photos goes to one with one click. The arrows and the arrow keys go to the next and the last photo. Under the
     * photo are its place, its time of day and "Remove", which asks no question. The frame of the photo has one size, so
     * no part of the window moves from one photo to the next. */
    let { library, photos, first, onClose }: {
        library: Library;
        /** The photos in the order of the ride: the address of the small picture, and the place with the time of day. */
        photos: { id: string; url: string; text: string }[];
        /** The photo that shows first. */
        first: string;
        onClose: () => void;
    } = $props();

    /** After "Remove" the index stays, so the photo after the removed one shows. */
    let index = $state(untrack(() => Math.max(0, photos.findIndex(photo => photo.id === first))));
    const at = $derived(Math.min(index, photos.length - 1));
    const shown = $derived(photos.at(at));
    /** The copy of the photo that shows. Until the library gives it, the small picture stands in its place. */
    let copy = $state.raw<{ id: string; url: string }>();
    let list = $state<HTMLElement>(), failure = $state('');

    const go = (to: number) => { index = Math.max(0, Math.min(photos.length - 1, to)); };

    $effect(() => {
        const id = shown?.id;
        if (!id) return onClose();
        let live = true, url: string | undefined;
        void library.photo(id).then(record => {
            if (live && record) copy = { id, url: url = URL.createObjectURL(record.image) };
        });
        return () => { live = false; if (url) URL.revokeObjectURL(url); };
    });

    // The photo that shows is in view in the list.
    $effect(() => {
        void shown?.id;
        list?.querySelector('[aria-current="true"]')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    });

    function key(event: KeyboardEvent) {
        const step = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 0;
        if (!step) return;
        event.preventDefault();
        go(at + step);
    }

    async function remove() {
        failure = '';
        try { await library.removePhoto(shown!.id); } catch (reason) { failure = reason instanceof Error ? reason.message : String(reason); }
    }
</script>

<svelte:window onkeydown={key} />

{#if shown}
    <Window title={`Photo ${at + 1} of ${photos.length}`} wide {onClose}>
        <div class="photos">
            <ul bind:this={list} aria-label="Photos of the ride">
                {#each photos as photo, i (photo.id)}
                    <li><button type="button" aria-label={`Photo ${i + 1}`} aria-current={i === at} onclick={() => go(i)}><img src={photo.url} alt="" /></button></li>
                {/each}
            </ul>
            <button type="button" class="step" aria-label="Photo before" disabled={at === 0} onclick={() => go(at - 1)}><span class="back"><Icon name="chevron" /></span></button>
            <div class="frame"><img src={copy?.id === shown.id ? copy.url : shown.url} alt={`Photo ${at + 1} of ${photos.length}`} /></div>
            <button type="button" class="step" aria-label="Next photo" disabled={at === photos.length - 1} onclick={() => go(at + 1)}><Icon name="chevron" /></button>
            <div class="foot">
                <span>{shown.text}</span>
                {#if failure}<span class="failure" role="alert">{failure}</span>{/if}
                <button type="button" class="remove" onclick={remove}>Remove</button>
            </div>
        </div>
    </Window>
{/if}

<style>
    /* The list, an arrow, the photo, an arrow; the foot stands under the photo. */
    .photos {
        display: grid;
        grid-template-columns: 72px 36px minmax(0, 1fr) 36px;
        grid-template-rows: min(600px, calc(100dvh - 190px)) auto;
        column-gap: 6px;
    }
    ul {
        display: flex;
        flex-direction: column;
        gap: 6px;
        min-height: 0;
        /* Room for the edge of the photo that shows; the pictures stand on the left edge of the title. */
        margin: -3px;
        padding: 3px;
        overflow-y: auto;
        list-style: none;
        /* The list follows the photo that shows, and a cut picture at its end says that it goes on. */
        scrollbar-width: none;
    }
    li {
        flex: none;
    }
    ul button {
        display: block;
        width: 100%;
        padding: 0;
        border: 0;
        border-radius: 5px;
        background: var(--parchment-2);
    }
    ul img {
        display: block;
        width: 100%;
        aspect-ratio: 4 / 3;
        border-radius: 5px;
        object-fit: cover;
    }
    ul button[aria-current="true"] {
        outline: 2px solid var(--ink);
        outline-offset: 1px;
    }
    ul button:focus-visible {
        outline: 2px solid var(--ink-soft);
        outline-offset: 1px;
    }
    .frame {
        min-width: 0;
        min-height: 0;
        overflow: hidden;
        border-radius: 8px;
        background: var(--parchment-2);
    }
    .frame img {
        display: block;
        width: 100%;
        height: 100%;
        object-fit: contain;
    }
    .step {
        display: grid;
        place-items: center;
        align-self: center;
        width: 36px;
        height: 36px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
    }
    .step:not(:disabled):hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .step:disabled {
        color: var(--line-strong);
    }
    .back {
        display: grid;
        rotate: 180deg;
    }
    .foot {
        display: flex;
        grid-column: 3;
        gap: 12px;
        align-items: baseline;
        padding-top: 12px;
        color: var(--ink-soft);
        font-size: 13px;
        font-variant-numeric: tabular-nums;
    }
    .failure {
        color: var(--coral);
    }
    /* Coral text with no line, at the far end. */
    .remove {
        margin: -4px -8px -4px auto;
        padding: 4px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--coral);
        font-size: 13px;
    }
    .remove:hover {
        background: var(--parchment-2);
    }
    /* On a phone the photo has the width of the screen, and the list is a row under the foot: the row replaces the arrows. */
    @media (max-width: 760px) {
        .photos {
            grid-template-columns: minmax(0, 1fr);
            grid-template-rows: min(62dvh, 100vw) auto auto;
        }
        .step {
            display: none;
        }
        .frame {
            grid-row: 1;
        }
        .foot {
            grid-row: 2;
            grid-column: 1;
        }
        ul {
            flex-direction: row;
            grid-row: 3;
            margin-top: 11px;
            overflow: auto hidden;
        }
        ul button {
            width: 64px;
        }
    }
</style>
