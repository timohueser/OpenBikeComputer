<script lang="ts">
    import Switch from '../lib/ui/Switch.svelte';
    import Window from '../lib/ui/Window.svelte';
    import Detail from '../components/route/Detail.svelte';
    import AddPhotos from '../components/route/AddPhotos.svelte';
    import Menu from '../lib/ui/Menu.svelte';
    import Photos from '../components/route/Photos.svelte';
    import Picture from '../components/route/Picture.svelte';
    import RideFacts from '../components/route/RideFacts.svelte';
    import RideLines from '../components/route/RideLines.svelte';
    import Share from '../components/route/Share.svelte';
    import { download } from '../components/route/download';
    import { writeFitRide, type FitSample } from '../lib/core/fit';
    import { writeGpx } from '../lib/core/gpx';
    import type { RideRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { lineCoordinates } from '../lib/library/summary';
    import { rideAgain } from '../lib/ride/again';
    import { distanceAxis, lineShares, slice } from '../lib/ride/lines';
    import { channels, movingSpeed, partNumbers, rideClimbs, rideCsv, type PartNumbers } from '../lib/ride/numbers';
    import { photoSample } from '../lib/ride/photos';
    import { fitSport, ridePoints, rideSamples } from '../lib/ride/samples';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { formatAlong, formatClock } from '../lib/settings/format';
    import { settings } from '../lib/settings/settings.svelte';

    let { library, active }: { library: Library; active: boolean } = $props();

    const key = $derived(router.id ?? '');
    let record = $state.raw<RideRecord | null>();
    /** The source file and its samples; null when the file cannot be read. */
    let source = $state.raw<{ file: Blob; samples: FitSample[] } | null>();
    /** The pointer, as a share of the summary line from 0 to 1. */
    let share = $state<number>();
    let deleting = $state(false), sharing = $state(false), failure = $state('');
    /** The selected part, from its first to its last sample, and its numbers. A drag changes the part with each move:
     * the numbers follow PART_MS after the last one. */
    let part = $state.raw<[number, number] | null>(null), partFacts = $state.raw<PartNumbers>();
    const PART_MS = 120;
    /** The exports hold only the selected part. */
    let onlyPart = $state(false);
    /** The photos of the ride in its order: the address of the small picture, the sample of the place, and the time
     * when the photo has one. */
    let photos = $state.raw<{ id: string; url: string; sample: number; time: number | null }[]>([]);
    /** The files that the window "Add photos" reads, and the photo that the photo window opens with. */
    let chosen = $state.raw<File[]>(), opened = $state<string>();
    let picker = $state<HTMLInputElement>(), dropping = $state(false);

    const summary = $derived(record?.summary);
    const samples = $derived(source?.samples);
    // The values that the ride brought, else those of the rider now.
    const limits = $derived({ maxHr: record?.maxHr ?? settings.maxHr?.value ?? null, ftp: record?.ftp ?? settings.ftp?.value ?? null });
    const numbers = $derived(samples && channels(samples, limits));
    const shares = $derived(samples && summary && lineShares(samples, lineCoordinates(summary)));
    /** The part, as shares of the summary line. */
    const lit = $derived<[number, number] | null>(part && shares ? [shares[part[0]], shares[part[1]]] : null);
    const climbs = $derived(samples && rideClimbs(samples));
    const speed = $derived(samples && movingSpeed(samples));
    const date = $derived(summary && new Date(summary.start * 1000).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', year: 'numeric' }));
    const marks = $derived(shares ? photos.map(({ id, url, sample }) => ({ id, url, share: shares[sample] })) : []);
    /** For the photo window: the place of each photo on the distance axis, and its time of day. */
    const listed = $derived.by(() => {
        if (!samples) return [];
        const { xs, lengthM } = distanceAxis(samples);
        return photos.map(({ id, url, sample, time }) => ({ id, url, text: [formatAlong(xs[sample] * lengthM), ...time === null ? [] : [formatClock(time)]].join(' · ') }));
    });

    // The record loads again after each write of this ride, the file only for a new key.
    $effect(() => {
        const target = key;
        let live = true, reading = false;
        record = source = undefined;
        [share, deleting, sharing, failure, part, onlyPart, chosen, opened] = [undefined, false, false, '', null, false, undefined, undefined];
        const load = async () => {
            const ride = await library.ride(target);
            if (!live) return;
            record = ride ?? null;
            if (!ride || reading) return;
            reading = true;
            const file = await library.rideFile(target);
            const read = file && await rideSamples(ride, file).catch(() => undefined);
            if (live) source = file && read?.length ? { file, samples: read } : null;
        };
        void load();
        const stop = library.onChange((store, changed) => { if (store === 'rides' && changed === target) void load(); });
        return () => { live = false; stop(); };
    });

    $effect(() => {
        const [range, read, zones] = [part, samples, limits];
        if (!range || !read) return void (partFacts = undefined);
        const timer = setTimeout(() => { partFacts = partNumbers(read, range, zones); }, PART_MS);
        return () => clearTimeout(timer);
    });

    // The photos load again after each photo that this tab adds or removes. A small picture keeps its address while
    // its photo is in the list.
    $effect(() => {
        const target = key, read = samples, urls = new Map<string, string>();
        if (!read) return void (photos = []);
        let live = true;
        const load = async () => {
            const records = await library.photos(target);
            if (!live) return;
            const next = records.flatMap(record => {
                const sample = photoSample(read, record);
                if (sample === undefined) return [];
                if (!urls.has(record.id)) urls.set(record.id, URL.createObjectURL(record.small));
                return [{ id: record.id, url: urls.get(record.id)!, sample, time: record.time }];
            }).sort((a, b) => a.sample - b.sample || (a.time ?? 0) - (b.time ?? 0));
            for (const [id, url] of urls) {
                if (next.some(photo => photo.id === id)) continue;
                URL.revokeObjectURL(url);
                urls.delete(id);
            }
            photos = next;
        };
        void load();
        const stop = library.onChange(store => { if (store === 'photos') void load(); });
        return () => { live = false; stop(); urls.forEach(url => URL.revokeObjectURL(url)); };
    });

    /** A drag of files while the page shows. The page takes each one, so the browser never opens a dropped file in
     * place of the app; it adds the files only with a ride that is read and with no window open. */
    function filesOver(event: DragEvent) {
        if (!active || !event.dataTransfer?.types.includes('Files')) return false;
        event.preventDefault();
        return !!samples && !document.querySelector('dialog:modal');
    }

    function drop(event: DragEvent) {
        dropping = false;
        if (filesOver(event)) chosen = [...event.dataTransfer!.files];
    }

    /** Runs a library action; its error shows under the title. */
    async function act(work: () => Promise<unknown>) {
        failure = '';
        try { await work(); } catch (reason) { failure = reason instanceof Error ? reason.message : String(reason); }
    }

    const rename = (name: string) => act(() => library.saveRide({ ...record!, name }));

    function remove() {
        deleting = false;
        return act(async () => {
            await library.removeRide(key);
            router.replace(`${ROUTES.routes}/ridden`);
        });
    }

    /** Share, "Add photos", the exports, then Delete. An export of the whole ride is the stored file for its own format, else a file
     * written from the samples. With a part, the switch "Only the selected part" stands over the exports: with it on,
     * each export is written from the samples of the part, and its name says so. The share card is the whole ride. */
    const more = $derived.by(() => {
        const ride = record, read = source, range = onlyPart ? part : null;
        const samplesOf = () => range ? slice(read!.samples, range) : read!.samples, name = () => range ? `${ride!.name} part` : ride!.name;
        const file = (format: RideRecord['format'], write: () => BlobPart) => () => {
            if (!ride || !read) return;
            const [type, extension] = format === 'fit' ? ['application/vnd.ant.fit', 'fit'] : ['application/gpx+xml', 'gpx'];
            download(ride.format === format && !range ? read.file : write(), type, name(), extension);
        };
        return [
            { label: 'Share', disabled: !ride, run: () => { sharing = true; } },
            { label: 'Add photos', disabled: !read, run: () => picker?.click() },
            { label: 'Export FIT file', rule: true, control: part ? partSwitch : undefined, disabled: !read, run: file('fit', () => writeFitRide({ sport: fitSport(ride!.summary.bike), samples: samplesOf() }) as Uint8Array<ArrayBuffer>) },
            { label: 'Export GPX track', disabled: !read, run: file('gpx', () => writeGpx({ name: name(), points: ridePoints(samplesOf()), waypoints: [] })) },
            { label: 'Export table of the data (CSV)', note: 'One line for each point', disabled: !read,
                run: () => { if (ride && read) download(rideCsv(samplesOf()), 'text/csv', name(), 'csv'); } },
            { label: 'Delete', rule: true, danger: true, run: () => { deleting = true; } },
        ];
    });
</script>

{#snippet partSwitch()}<Switch label="Only the selected part" bind:checked={onlyPart} />{/snippet}

<svelte:window ondragover={event => { dropping = filesOver(event); }} ondrop={drop} ondragleave={event => { if (!event.relatedTarget) dropping = false; }} />
<input bind:this={picker} type="file" accept="image/*" multiple hidden
    onchange={event => { const files = [...event.currentTarget.files ?? []]; event.currentTarget.value = ''; if (files.length) chosen = files; }} />

<Detail message={record === undefined ? 'Loading…' : record === null ? 'This ride is not in the library.' : undefined}
    name={record?.name ?? ''} nameLabel="Ride name" onrename={rename} {failure}>
    {#snippet tags()}
        <span class="date">{date}</span>
    {/snippet}
    {#snippet buttons()}
        <button type="button" class="button" disabled={!samples} onclick={() => act(() => rideAgain(library, record!, samples))}>Ride it again</button>
        <Menu label="More" items={more} />
    {/snippet}
    {#snippet left()}
        {#if summary}
            <RideFacts {summary} {speed} channels={numbers} {climbs} part={partFacts} unreadable={source === null}
                onclimb={climb => part = [climb.first, climb.last]} />
        {/if}
    {/snippet}
    {#snippet picture()}
        {#if summary}<Picture {library} id={key} {summary} ride photos={marks} onphoto={id => opened = id} mark={share} onpoint={next => share = next ?? undefined} part={lit} {lit} />{/if}
    {/snippet}
    {#if samples && shares}
        {#key key}<RideLines {samples} {shares} {speed} channels={numbers} {climbs} {limits} {photos} onphoto={id => opened = id} mark={share}
            onpoint={next => share = next ?? undefined} {part} onpart={next => part = next} />{/key}
    {/if}
    {#if dropping}<p class="drop">Drop to add photos</p>{/if}
    {#if chosen && samples}<AddPhotos {library} ride={key} {samples} files={chosen} onClose={() => chosen = undefined} />{/if}
    {#if opened}<Photos {library} ride={key} card={record?.photo} photos={listed} first={opened} onClose={() => opened = undefined} />{/if}
    {#if sharing && record}<Share {library} id={key} name={record.name} summary={record.summary} {date} onClose={() => sharing = false} />{/if}
    {#if deleting}
        <Window title="Delete this ride?" onClose={() => deleting = false}>
            <p>“{record?.name}” and its file leave the library. You cannot undo this.</p>
            <div class="window-buttons">
                <button type="button" class="button" onclick={() => deleting = false}>Cancel</button>
                <button type="button" class="button danger" onclick={remove}>Delete</button>
            </div>
        </Window>
    {/if}
</Detail>

<style>
    .date {
        flex: none;
        color: var(--ink-soft);
    }
    /* The plate of the pointer, at the foot of the window, while files are over the page. */
    .drop {
        position: fixed;
        bottom: 24px;
        left: 50%;
        z-index: 5;
        margin: 0;
        padding: 6px 12px;
        border-radius: 6px;
        background: var(--ink);
        color: var(--panel);
        font: 600 13px/1.4 var(--sans);
        translate: -50%;
        pointer-events: none;
    }
</style>
