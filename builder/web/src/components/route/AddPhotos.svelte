<script lang="ts">
    import { onMount } from 'svelte';
    import Window from '../../lib/ui/Window.svelte';
    import type { FitSample } from '../../lib/core/fit';
    import { photoFacts } from '../../lib/library/photo-time';
    import { addPhoto, smallPicture } from '../../lib/library/photos';
    import type { Library } from '../../lib/library/store';
    import { placePhoto, type PhotoPlace } from '../../lib/ride/photos';

    /** The window "Add photos": it reads the chosen files, shows the photos that are of the ride as small pictures, and
     * says how many are not and how many the browser cannot open. "Add" then stores one photo at a time. A close of the
     * window stops the work; the photos that are stored stay. */
    let { library, ride, samples, files, onClose }: {
        library: Library;
        /** The key of the ride. */
        ride: string;
        samples: FitSample[];
        files: File[];
        onClose: () => void;
    } = $props();

    /** The photos of the ride, with the address of a small picture. */
    let found = $state.raw<{ file: File; at: PhotoPlace; url: string }[]>([]);
    /** Files read; photos that are not of the ride; other files that the browser cannot open; HEIC files that it cannot open. */
    let read = $state(0), outside = $state(0), closed = $state(0), heic = $state(0);
    /** Photos stored, while "Add" works. */
    let added = $state<number>();
    let failure = $state('');

    const count = (n: number, noun = 'photo') => `${n} ${noun}${n === 1 ? '' : 's'}`;
    const reading = $derived(read < files.length);

    /** False after the window closes: the work on the files stops at the next file. */
    let live = true;

    // One file at a time: the memory holds one open photo.
    onMount(() => {
        const urls: string[] = [];
        void (async () => {
            for (const file of files) {
                const at = placePhoto(samples, await photoFacts(file));
                // A JPEG file with no place needs no try: the browser opens it. Another kind of file can be a photo
                // with no time, or a file that the browser cannot open.
                const small = at || file.type !== 'image/jpeg' ? await smallPicture(file).catch(() => null) : undefined;
                if (!live) return;
                if (small === null) {
                    if (/hei[cf]/.test(file.type) || /\.hei[cf]$/i.test(file.name)) heic++;
                    else closed++;
                } else if (at && small) {
                    urls.push(URL.createObjectURL(small));
                    found = [...found, { file, at, url: urls.at(-1)! }];
                } else outside++;
                read++;
            }
        })();
        return () => { live = false; urls.forEach(url => URL.revokeObjectURL(url)); };
    });

    async function add(close: () => void) {
        failure = '';
        added = 0;
        try {
            for (const { file, at } of found) {
                if (!live) return;
                await addPhoto(library, ride, file, at);
                added++;
            }
            close();
        } catch (reason) {
            failure = reason instanceof DOMException && reason.name === 'QuotaExceededError'
                ? `This browser has no room for more photos. ${count(added)} of ${found.length} ${added === 1 ? 'is' : 'are'} added.`
                : reason instanceof Error ? reason.message : String(reason);
        }
    }
</script>

<Window title="Add photos" {onClose}>
    {#snippet children(close)}
        <div class="lines" aria-live="polite">
            {#if reading}
                <p class="quiet">Reading {count(files.length)}…</p>
            {:else}
                {#if found.length}
                    <p><strong>{count(found.length)} {found.length === 1 ? 'is' : 'are'} of this ride.</strong>
                        {#if outside}{outside} {outside === 1 ? 'is not: it has no time, or its time is' : 'are not: they have no time, or their time is'} not in the ride.{/if}</p>
                {:else if outside}
                    <p><strong>{outside === 1 ? 'This photo is' : `These ${outside} photos are`} not of this ride.</strong>
                        {outside === 1 ? 'It has no time, or its time is' : 'They have no time, or their time is'} not in the ride.</p>
                {/if}
                {#if heic}
                    <p><strong>This browser cannot open {count(heic)}.</strong>
                        {heic === 1 ? 'It is a HEIC file. Export it as JPEG.' : 'They are HEIC files. Export them as JPEG.'}</p>
                {/if}
                {#if closed}
                    <p><strong>This browser cannot open {count(closed, 'file')}.</strong>
                        {closed === 1 ? 'It is not a photo, or it is damaged.' : 'They are not photos, or they are damaged.'}</p>
                {/if}
            {/if}
        </div>
        {#if found.length}
            <ul class="found">
                {#each found as photo (photo.url)}<li><img src={photo.url} alt="" /></li>{/each}
            </ul>
        {/if}
        {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
        <div class="window-buttons">
            {#if found.length && !failure}
                <button type="button" class="button" onclick={close}>Cancel</button>
                <button type="button" class="button primary" disabled={reading || added !== undefined} onclick={() => add(close)}>
                    {added === undefined ? `Add ${count(found.length)}` : `Adding ${Math.min(added + 1, found.length)} of ${found.length}…`}
                </button>
            {:else}
                <button type="button" class="button" onclick={close}>Close</button>
            {/if}
        </div>
    {/snippet}
</Window>

<style>
    .lines {
        display: grid;
        gap: 6px;
    }
    .lines p {
        margin: 0;
        color: var(--ink-soft);
    }
    .lines strong {
        color: var(--ink);
        font-weight: 600;
    }
    /* Six small pictures in a row. More than four rows scroll, and the cut fifth row says so. */
    .found {
        display: grid;
        grid-template-columns: repeat(6, minmax(0, 1fr));
        gap: 6px;
        max-height: 294px;
        margin: 14px 0 0;
        padding: 0;
        overflow-y: auto;
        list-style: none;
    }
    .found img {
        display: block;
        width: 100%;
        aspect-ratio: 4 / 3;
        border-radius: 5px;
        background: var(--parchment-2);
        object-fit: cover;
    }
    .failure {
        margin-top: 14px;
        color: var(--coral);
    }
    .primary {
        font-variant-numeric: tabular-nums;
    }
    @media (max-width: 760px) {
        .found {
            grid-template-columns: repeat(4, minmax(0, 1fr));
        }
    }
</style>
