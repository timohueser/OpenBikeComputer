<script lang="ts">
    import Window from '../lib/ui/Window.svelte';
    import Detail from '../components/route/Detail.svelte';
    import Menu from '../components/route/Menu.svelte';
    import Picture from '../components/route/Picture.svelte';
    import RideFacts from '../components/route/RideFacts.svelte';
    import { download } from '../components/route/download';
    import { writeFitRide, type FitSample } from '../lib/core/fit';
    import { writeGpx } from '../lib/core/gpx';
    import type { RideRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { rideAgain } from '../lib/ride/again';
    import { channels, movingSpeed, rideClimbs, rideCsv } from '../lib/ride/numbers';
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

    const summary = $derived(record?.summary);
    const samples = $derived(source?.samples);
    // The values that the ride brought, else those of the rider now.
    const numbers = $derived(samples && record ? channels(samples, { maxHr: record.maxHr ?? settings.maxHr?.value ?? null, ftp: record.ftp ?? settings.ftp?.value ?? null }) : undefined);
    const climbs = $derived(samples && rideClimbs(samples));
    const speed = $derived(samples && movingSpeed(samples));
    const date = $derived(summary && new Date(summary.start * 1000).toLocaleDateString('en-GB', { day: 'numeric', month: 'long', year: 'numeric' }));

    // The record loads again after each write of this ride, the file only for a new key.
    $effect(() => {
        const target = key;
        let live = true, reading = false;
        record = source = undefined;
        [share, deleting, failure] = [undefined, false, ''];
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
    {/snippet}
    {#snippet left()}
        {#if summary}
            <RideFacts {summary} {speed} channels={numbers} {climbs} unreadable={source === null}>
                <button type="button" class="button danger" onclick={() => deleting = true}>Delete</button>
            </RideFacts>
        {/if}
    {/snippet}
    {#snippet picture()}
        {#if summary}<Picture {library} id={key} {summary} ride mark={share} onpoint={next => share = next ?? undefined} />{/if}
    {/snippet}
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
</style>
