<!-- The window "Add files": it reads GPX and FIT files, photos, folders and ZIP exports, counts the rides, the planned
     routes and the photos in them, and adds them to the library. A photo goes to the ride of its time, a stored ride or
     one of the files. -->
<script lang="ts">
    import { onDestroy } from 'svelte';
    import { readFit, type FitSample } from '../lib/core/fit';
    import { readGpx } from '../lib/core/gpx';
    import { readFiles, reasons, type Found, type Item, type Readers } from '../lib/library/import';
    import { photoFacts } from '../lib/library/photo-time';
    import { addPhoto, isHeic, smallPicture } from '../lib/library/photos';
    import type { RideRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { planFromFile } from '../lib/planner/gpx-import';
    import { photoRide, type PhotoRide } from '../lib/ride/photos';
    import { rideSamples } from '../lib/ride/samples';
    import { formatDistance } from '../lib/settings/format';
    import Segmented from '../lib/ui/Segmented.svelte';
    import Window from '../lib/ui/Window.svelte';

    let { library, onClose }: { library: Library; onClose: () => void } = $props();

    const readers: Readers = { gpx: readGpx, fit: readFit };
    const KINDS = [
        { kind: 'ride', one: 'ride', label: 'rides' },
        { kind: 'route', one: 'planned route', label: 'planned routes' },
        { kind: 'photo', one: 'photo', label: 'photos' },
        { kind: 'known', one: 'is in your library already', label: 'are in your library already' },
        { kind: 'unreadable', one: 'not readable', label: 'not readable' },
    ] as const;

    // `found` is not reactive: a large export has thousands of records. `shown` follows `version`.
    const found: Found[] = [];
    /** The files that are added, so that "Add" after an error goes on with the others. */
    const done = new Set<Found>();
    let version = $state(0);
    let counts = $state({ ride: 0, route: 0, photo: 0, known: 0, unreadable: 0 });
    /** The names of the photos that fit no ride. */
    let unplaced = $state<string[]>([]);
    /** The rides that a photo can be of, made at the first photo of a read. */
    let rides: Promise<(PhotoRide & { record: RideRecord })[]> | undefined;
    let source = $state('');
    let phase = $state<'empty' | 'reading' | 'read' | 'adding' | 'added'>('empty');
    let added = $state({ ride: 0, route: 0, photo: 0 });
    let message = $state('');
    let expanded = $state(false);
    /** A file that can be a ride or a planned route is in the list. */
    let choice = $state(false);
    let over = $state(false);
    let input: HTMLInputElement;
    let controller = new AbortController();

    const total = $derived(counts.ride + counts.route + counts.photo + counts.known + counts.unreadable + unplaced.length);
    const toAdd = $derived(counts.ride + counts.route + counts.photo - added.ride - added.route - added.photo);
    const shown = $derived((void version, expanded ? found.slice() : found.slice(0, 4)));
    const hidden = $derived((void version, found.length - shown.length));

    onDestroy(() => controller.abort());

    async function read(items: Item[]): Promise<void> {
        if (!items.length || phase === 'adding') return;
        controller.abort();
        const current = controller = new AbortController();
        found.length = 0;
        done.clear();
        counts = { ride: 0, route: 0, photo: 0, known: 0, unreadable: 0 };
        added = { ride: 0, route: 0, photo: 0 };
        unplaced = [];
        rides = undefined;
        source = items.length === 1 ? items[0].name : '';
        message = '';
        expanded = choice = false;
        phase = 'reading';
        version++;
        try {
            for await (const read of readFiles(items, library, readers, current.signal)) {
                const file = read.kind === 'photo' ? await place(read) : read;
                // A file of a run that a new drop or Cancel stopped.
                if (current.signal.aborted) return;
                if (!file) { unplaced.push(read.name); continue; }
                found.push(file);
                counts[file.kind]++;
                if ('other' in file && file.other) choice = true;
                version++;
            }
        } catch (error) {
            if (current.signal.aborted) return;
            message = error instanceof Error ? error.message : String(error);
        }
        phase = 'read';
    }

    /** The stored rides and the rides of the files, which come before the photos. Each file is read at most once. */
    async function photoRides(): Promise<(PhotoRide & { record: RideRecord })[]> {
        const ride = (record: RideRecord, file: () => Promise<Blob | undefined>) => {
            let samples: Promise<FitSample[] | undefined> | undefined;
            return { record, summary: record.summary, samples: () => samples ??= file().then(blob => blob && rideSamples(record, blob)).catch(() => undefined) };
        };
        return [...(await library.rides()).map(record => ride(record, () => library.rideFile(record.key))),
            ...found.flatMap(file => file.kind === 'ride' ? [ride(file.ride, file.open)] : [])];
    }

    /** A photo with its ride, a file that the browser cannot open, or undefined for a photo that fits no ride. */
    async function place(file: Found & { kind: 'photo' }): Promise<Found | undefined> {
        const unreadable = (reason: string): Found => ({ file: file.file, name: file.name, kind: 'unreadable', reason });
        const blob = await file.open().catch(() => undefined);
        if (!blob) return unreadable(reasons.damaged);
        const match = await photoRide(await (rides ??= photoRides()), await photoFacts(blob));
        if (!match) return undefined;
        // The browser opens each photo of a ride now, so that "Add" fails on none.
        if (!await smallPicture(blob).then(() => true, () => false)) return unreadable(isHeic({ name: file.name, type: blob.type }) ? reasons.heic : reasons.damaged);
        return { ...file, ride: { key: match.ride.record.key, name: match.ride.record.name, at: match.at } };
    }

    function drop(event: DragEvent): void {
        event.preventDefault();
        over = false;
        // The items of a drop can be read only during the event.
        const items = [...event.dataTransfer?.items ?? []].flatMap((item): Item[] => {
            if (item.kind !== 'file') return [];
            const entry = item.webkitGetAsEntry(), file = entry ? null : item.getAsFile();
            return entry ? [entry] : file ? [file] : [];
        });
        void read(items);
    }

    /** Writes one by one. A false from the library means another tab or an earlier file added the same record. */
    async function add(): Promise<void> {
        phase = 'adding';
        message = '';
        const { signal } = controller;
        for (let i = 0; i < found.length && !signal.aborted; i++) {
            const file = found[i];
            if (file.kind !== 'ride' && file.kind !== 'route' && file.kind !== 'photo' || done.has(file)) continue;
            let ok: boolean;
            try {
                ok = file.kind === 'ride' ? await library.addRide(file.ride, await file.open())
                    : file.kind === 'route' ? await library.addRoute(await planFromFile(await file.open(), file.id, file.bike))
                    : await addPhoto(library, file.ride!.key, await file.open(), file.ride!.at);
            } catch (error) {
                message = error instanceof Error ? error.message : String(error);
                phase = 'read';
                return;
            }
            done.add(file);
            if (ok) added[file.kind]++;
            else {
                counts[file.kind]--;
                counts.known++;
                found[i] = { file: file.file, name: file.name, kind: 'known' };
                version++;
            }
        }
        if (!signal.aborted) phase = 'added';
    }

    const FORMS = [{ value: 'ride', label: 'ride' }, { value: 'route', label: 'planned route' }] as const;

    /** A GPX file with times becomes a ride or a planned route, as the rider chooses. */
    function swap(i: number): void {
        const file = found[i];
        if (file.kind !== 'ride' && file.kind !== 'route' || !file.other) return;
        counts[file.kind]--;
        counts[file.other.kind as 'ride' | 'route']++;
        found[i] = file.other;
        version++;
    }

    const plural = (n: number, one: string, many: string) => `${n.toLocaleString('en')} ${n === 1 ? one : many}`;
    const and = (parts: string[]) => parts.length > 1 ? `${parts.slice(0, -1).join(', ')} and ${parts.at(-1)}` : parts[0];
    const unplacedNames = $derived(and(unplaced.length > 3 ? [...unplaced.slice(0, 2), `${unplaced.length - 2} more`] : unplaced));

    const NAMES = { ride: 'ride', route: 'planned route' };
    const lengthOf = (file: Found & { kind: 'ride' | 'route' }) => formatDistance(file.kind === 'ride' ? file.ride.summary.lengthM : file.lengthM);
</script>

<Window title="Add files" {onClose}>
    {#snippet children(close)}
        <div
            class="drop"
            class:small={phase !== 'empty'}
            class:over
            role="region"
            aria-label="Drop files or a folder"
            ondragover={event => { event.preventDefault(); over = true; }}
            ondragleave={() => over = false}
            ondrop={drop}
        >
            <strong>Drop files or a folder here</strong>
            {#if phase === 'empty'}<p>GPX and FIT files, photos of your rides, or the ZIP file with all your data from Strava, Komoot or Garmin.</p>{/if}
            <button type="button" onclick={() => input.click()} disabled={phase === 'adding'}>Choose files</button>
            <input bind:this={input} type="file" multiple hidden aria-label="Choose files"
                onchange={() => { void read([...input.files ?? []]); input.value = ''; }} />
        </div>

        {#if phase !== 'empty'}
            <section aria-label="Files read">
                <p class="head"><strong>{plural(total, 'file', 'files')} read</strong>{#if source}<span>from {source}</span>{/if}{#if phase === 'reading'}<span>reading…</span>{/if}</p>
                <div class="bar" aria-hidden="true">
                    {#each KINDS as { kind } (kind)}<span class={kind} style:flex-grow={counts[kind]}></span>{/each}
                </div>
                <dl class="counts">
                    {#each KINDS as { kind, one, label } (kind)}
                        <div class={kind} class:none={counts[kind] === 0}><dt>{counts[kind] === 1 ? one : label}</dt><dd>{counts[kind].toLocaleString('en')}</dd></div>
                    {/each}
                </dl>
                {#if choice}<p class="rule">A file with times comes in as a ride. You can change it to a planned route.</p>{/if}
                {#if unplaced.length}<p class="rule">{unplacedNames} {unplaced.length === 1 ? 'fits no ride. It is' : 'fit no ride. They are'} not added.</p>{/if}
                {#if shown.length}
                    <ul class:expanded>
                        {#each shown as file, i (i)}
                            <li>
                                <span class="name" title={file.file}>{file.name}</span>
                                {#if file.kind === 'known'}
                                    <span class="detail">in your library already</span>
                                {:else if file.kind === 'unreadable'}
                                    <span class="detail unreadable">not readable: {file.reason}</span>
                                {:else if file.kind === 'photo'}
                                    <span class="detail">photo of {file.ride?.name}</span>
                                {:else}
                                    {#if file.other && (phase === 'reading' || phase === 'read') && !done.has(file)}
                                        <Segmented compact label={`${file.name}: ride or planned route`} options={[...FORMS]} value={file.kind} onChange={() => swap(i)} />
                                    {:else}
                                        <span class="detail">{NAMES[file.kind]}</span>
                                    {/if}
                                    <span class="detail length">{lengthOf(file)}</span>
                                {/if}
                            </li>
                        {/each}
                    </ul>
                    {#if hidden > 0}
                        <button type="button" class="more" onclick={() => expanded = true}>and {hidden.toLocaleString('en')} more ›</button>
                    {/if}
                {/if}
            </section>
        {/if}

        {#if message}<p class="message" role="alert">{message}</p>{/if}
        {#if phase === 'added'}
            <p class="message" role="status">Added {and([plural(added.ride, 'ride', 'rides'), plural(added.route, 'planned route', 'planned routes'),
                ...counts.photo ? [plural(added.photo, 'photo', 'photos')] : []])}.</p>
        {/if}

        <footer>
            {#if phase === 'added'}
                <button type="button" class="primary" onclick={close}>Close</button>
            {:else}
                <button type="button" onclick={() => { controller.abort(); close(); }}>Cancel</button>
                <button type="button" class="primary" onclick={add} disabled={phase !== 'read' || toAdd === 0}>
                    {phase === 'adding' ? `Adding… ${toAdd.toLocaleString('en')} left` : phase === 'empty' ? 'Add to the library' : `Add ${toAdd.toLocaleString('en')} to the library`}
                </button>
            {/if}
        </footer>
    {/snippet}
</Window>

<style>
    .drop {
        display: grid;
        justify-items: center;
        gap: 6px;
        padding: 20px 16px;
        border: 1px dashed var(--line-strong);
        border-radius: 10px;
        background: var(--parchment);
        text-align: center;
    }
    .drop.over { border-color: var(--forest); }
    .drop strong { font-size: 16px; }
    /* With files in the list the drop place is one low row, so the list and the button that adds lead. */
    .drop.small {
        grid-template-columns: 1fr auto;
        justify-items: start;
        align-items: center;
        padding: 8px 8px 8px 14px;
    }
    .drop.small strong { font-size: 14px; }
    .drop p {
        margin: 0 0 6px;
        color: var(--ink-soft);
        font-size: 13px;
    }
    button {
        padding: 6px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
    }
    button:hover:not(:disabled) { background: var(--parchment-2); }
    .primary, .primary:hover:not(:disabled) {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    section { margin-top: 16px; }
    .head {
        display: flex;
        flex-wrap: wrap;
        align-items: baseline;
        gap: 4px 8px;
        margin: 0 0 8px;
    }
    .head span {
        color: var(--ink-soft);
        font-size: 12px;
    }
    .bar {
        display: flex;
        height: 8px;
        overflow: hidden;
        border-radius: 4px;
        background: var(--parchment-2);
    }
    .bar span { flex-basis: 0; background: var(--kind); }
    .ride { --kind: var(--ride); }
    .route { --kind: var(--route); }
    .photo { --kind: var(--amber); }
    .known { --kind: var(--parchment-3); }
    .unreadable { --kind: var(--ink-faint); }
    /* One line under the bar: the colour, the number, the kind. A term comes before its value in the markup. */
    .counts {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 18px;
        margin: 8px 0 0;
        font-size: 13px;
    }
    .counts div {
        display: flex;
        flex-direction: row-reverse;
        align-items: center;
        gap: 5px;
    }
    .counts .none { display: none; }
    .counts div::after {
        content: "";
        width: 9px;
        height: 9px;
        margin-right: 1px;
        border-radius: 2px;
        background: var(--kind);
    }
    dd {
        margin: 0;
        font-weight: 700;
        font-variant-numeric: tabular-nums;
    }
    dt { color: var(--ink-soft); }
    .rule {
        margin: 14px 0 0;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    ul {
        margin: 10px 0 0;
        padding: 0;
        list-style: none;
    }
    ul.expanded {
        max-height: 40dvh;
        overflow: auto;
    }
    li {
        display: flex;
        justify-content: space-between;
        gap: 12px;
        align-items: center;
        min-height: 43px;
        padding: 6px 0;
        border-top: 1px solid var(--line);
        font-size: 13.5px;
    }
    .name {
        flex: 1;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .detail {
        flex: none;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    /* The lengths stand in one column at the right edge. */
    .length {
        min-width: 52px;
        font-variant-numeric: tabular-nums;
        text-align: right;
    }
    .detail.unreadable {
        color: var(--coral);
    }
    .more {
        padding: 8px 0 0;
        border: 0;
        border-top: 1px solid var(--line);
        border-radius: 0;
        width: 100%;
        background: none;
        color: var(--ink-soft);
        font-size: 12.5px;
        text-align: left;
    }
    .more:hover:not(:disabled) { background: none; color: var(--ink); }
    .message { margin: 12px 0 0; }
    footer {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        margin: 16px -20px 0;
        padding: 14px 20px 0;
        border-top: 1px solid var(--line);
    }
    @media (max-width: 520px) {
        .detail { max-width: 50%; text-align: right; }
        /* A row with the choice has the name on its own line. */
        li { flex-wrap: wrap; row-gap: 6px; }
        li:has(:global([role="radiogroup"])) .name { flex-basis: 100%; }
    }
</style>
