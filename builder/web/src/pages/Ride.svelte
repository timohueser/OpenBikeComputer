<script lang="ts">
    import Window from '../lib/ui/Window.svelte';
    import Detail from '../components/route/Detail.svelte';
    import Menu from '../components/route/Menu.svelte';
    import Picture from '../components/route/Picture.svelte';
    import RideFacts from '../components/route/RideFacts.svelte';
    import RideLines from '../components/route/RideLines.svelte';
    import { download } from '../components/route/download';
    import { writeFitRide, type FitSample } from '../lib/core/fit';
    import { writeGpx } from '../lib/core/gpx';
    import type { RideRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { lineCoordinates } from '../lib/library/summary';
    import { rideAgain } from '../lib/ride/again';
    import { lineShares } from '../lib/ride/lines';
    import { channels, movingSpeed, partNumbers, rideClimbs, rideCsv, type PartNumbers } from '../lib/ride/numbers';
    import { fitSport, ridePoints, rideSamples } from '../lib/ride/samples';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';
    import { settings } from '../lib/settings/settings.svelte';

    let { library }: { library: Library; active: boolean } = $props();

    const key = $derived(router.id ?? '');
    let record = $state.raw<RideRecord | null>();
    /** The source file and its samples; null when the file cannot be read. */
    let source = $state.raw<{ file: Blob; samples: FitSample[] } | null>();
    /** The pointer, as a share of the summary line from 0 to 1. */
    let share = $state<number>();
    let deleting = $state(false), failure = $state('');
    /** The selected part, from its first to its last sample, and its numbers. A drag changes the part with each move:
     * the numbers follow PART_MS after the last one. */
    let part = $state.raw<[number, number] | null>(null), partFacts = $state.raw<PartNumbers>();
    const PART_MS = 120;

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

    // The record loads again after each write of this ride, the file only for a new key.
    $effect(() => {
        const target = key;
        let live = true, reading = false;
        record = source = undefined;
        [share, deleting, failure, part] = [undefined, false, '', null];
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

    /** The stored file for its own format, else a file written from the samples. */
    const exports = $derived.by(() => {
        const ride = record, read = source;
        const file = (format: RideRecord['format'], write: () => BlobPart) => () => {
            if (!ride || !read) return;
            const [type, extension] = format === 'fit' ? ['application/vnd.ant.fit', 'fit'] : ['application/gpx+xml', 'gpx'];
            download(ride.format === format ? read.file : write(), type, ride.name, extension);
        };
        return [
            { label: 'FIT file', disabled: !read, run: file('fit', () => writeFitRide({ sport: fitSport(ride!.summary.bike), samples: read!.samples }) as Uint8Array<ArrayBuffer>) },
            { label: 'GPX track', disabled: !read, run: file('gpx', () => writeGpx({ name: ride!.name, points: ridePoints(read!.samples), waypoints: [] })) },
            { label: 'Table of the data (CSV)', note: 'One line for each point', disabled: !read,
                run: () => { if (ride && read) download(rideCsv(read.samples), 'text/csv', ride.name, 'csv'); } },
        ];
    });
</script>

<Detail message={record === undefined ? 'Loading…' : record === null ? 'This ride is not in the library.' : undefined}
    name={record?.name ?? ''} nameLabel="Ride name" onrename={rename} {failure}>
    {#snippet tags()}
        <span class="date">{date}</span>
    {/snippet}
    {#snippet buttons()}
        <button type="button" class="button" disabled={!samples} onclick={() => act(() => rideAgain(library, record!, samples))}>Ride it again</button>
        <Menu label="Export" items={exports} />
        <button type="button" class="remove" onclick={() => deleting = true}>Delete</button>
    {/snippet}
    {#snippet left()}
        {#if summary}
            <RideFacts {summary} {speed} channels={numbers} {climbs} part={partFacts} unreadable={source === null} />
        {/if}
    {/snippet}
    {#snippet picture()}
        {#if summary}<Picture {library} id={key} {summary} ride mark={share} onpoint={next => share = next ?? undefined} part={lit} {lit} />{/if}
    {/snippet}
    {#if samples && shares}
        {#key key}<RideLines {samples} {shares} {speed} channels={numbers} mark={share} onpoint={next => share = next ?? undefined}
            {part} onpart={next => part = next} />{/key}
    {/if}
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
    /* Beside other buttons, a delete is plain text at the far end. */
    .remove {
        padding: 4px 0 4px 4px;
        border: 0;
        background: none;
        color: var(--coral);
        font-size: 13px;
    }
    .remove:hover {
        text-decoration: underline;
    }
</style>
