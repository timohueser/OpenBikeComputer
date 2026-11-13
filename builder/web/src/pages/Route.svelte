<script lang="ts">
    import { untrack } from 'svelte';
    import Card from '../lib/ui/Card.svelte';
    import Window from '../lib/ui/Window.svelte';
    import Detail from '../components/route/Detail.svelte';
    import ExportMenu from '../components/route/ExportMenu.svelte';
    import Facts from '../components/route/Facts.svelte';
    import Picture from '../components/route/Picture.svelte';
    import Rows from '../components/route/Rows.svelte';
    import { download } from '../components/route/download';
    import { writeFitCourse } from '../lib/core/fit';
    import { writeGpx, type GpxImport } from '../lib/core/gpx';
    import type { RouteRecord, SentRoute } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { lineCoordinates, lineKey } from '../lib/library/summary';
    import { remembered } from '../lib/device-sync/remembered.svelte';
    import { hasEndpoints, planTitle, planView } from '../lib/planner/editor';
    import { downloadPlan, isPlan, isVersion, newPlan } from '../lib/planner/library';
    import { routeFacts, routePins } from '../lib/planner/route-facts';
    import { LegCache } from '../lib/planner/route-legs';
    import { calculateLine, routingKey, type RoutingLine } from '../lib/planner/routing';
    import type { Version } from '../lib/planner/versions';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';

    let { library }: { library: Library; active: boolean } = $props();

    const id = $derived(router.id ?? '');
    const legs = new LegCache();
    // Raw: `planView` freezes the trip that it reads.
    let record = $state.raw<RouteRecord | null>();
    let waiting = $state(false), sent = $state.raw<SentRoute>();
    let routed = $state.raw<{ key: string; line?: RoutingLine; error?: string }>();
    /** The pointer, as a share of the summary line from 0 to 1. */
    let share = $state<number>();
    let deleting = $state(false), failure = $state('');

    const plan = $derived(record && isPlan(record) ? record : undefined);
    const summary = $derived(record?.summary ?? null);
    const name = $derived(plan ? plan.name || planTitle(plan.trip) : record?.name ?? '');
    const key = $derived(plan && hasEndpoints(plan.trip) ? routingKey(plan.trip) : undefined);
    const line = $derived(routed?.key === key ? routed?.line : undefined);
    const error = $derived(!key ? 'This plan has no start and finish.' : routed?.key === key ? routed?.error : undefined);
    const facts = $derived(plan && line ? routeFacts(plan.trip, line) : undefined);
    const pins = $derived(plan && summary ? routePins(plan.trip, lineCoordinates(summary), line) : []);
    // Only an entry of the card of the remembered device counts.
    const sentHere = $derived(sent && remembered.device && sent.device.startsWith(`${remembered.device.storeId}:`) ? sent : undefined);
    // The device has the route as it is stored now.
    const onDevice = $derived(!!sentHere && !!record && sentHere.line === lineKey(record));
    const changed = $derived(!!plan && !!line && (!summary || Math.abs(planView(plan.trip, line).summary.distance * 1000 - summary.lengthM) > summary.lengthM / 100));

    $effect(() => {
        const target = id;
        let live = true;
        record = undefined;
        [share, deleting, failure] = [undefined, false, ''];
        const load = async () => {
            const [item, ids, routes] = await Promise.all([library.item(target), library.state('waiting'), library.state('sent')]);
            if (!live) return;
            record = item && 'revision' in item ? item : null;
            waiting = !!ids?.includes(target);
            sent = routes?.[target];
        };
        void load();
        const stop = library.onChange((store, changedKey) => {
            if (changedKey === target || (store === 'state' && (changedKey === 'waiting' || changedKey === 'sent'))) void load();
        });
        return () => { live = false; stop(); };
    });

    $effect(() => {
        const current = key;
        if (!current) return;
        const trip = untrack(() => plan!.trip), abort = new AbortController();
        calculateLine(trip, abort.signal, legs).then(
            found => { if (!abort.signal.aborted) routed = { key: current, line: found }; },
            reason => { if (!abort.signal.aborted) routed = { key: current, error: reason instanceof Error ? reason.message : 'Routing is unavailable.' }; });
        return () => abort.abort();
    });

    /** Runs a library action; its error shows under the title. */
    async function act(work: () => Promise<unknown>) {
        failure = '';
        try { await work(); } catch (reason) { failure = reason instanceof Error ? reason.message : String(reason); }
    }

    const rename = (next: string) => act(() => library.renameRoute(id, next));

    const send = (on: boolean) => act(() => library.changeState('waiting', ids => on ? [...(ids ?? []).filter(other => other !== id), id] : (ids ?? []).filter(other => other !== id)));

    async function versions(): Promise<Version[]> {
        return (await library.versions(id)).filter((v): v is Version => isVersion(v) && typeof v.summary === 'string');
    }

    function duplicate() {
        return act(async () => {
            const copy = await library.saveRoute({ ...newPlan(plan!.trip, `${name} (copy)`), summary }, await versions());
            location.hash = `${ROUTES.route}/${copy.id}`;
        });
    }

    function remove() {
        deleting = false;
        return act(async () => {
            await library.removeRoute(id);
            router.replace(ROUTES.routes);
        });
    }

    /** The calculated line with its waypoints, in the microdegrees of `lib/core`. */
    function course(): GpxImport {
        const micro = (value: number) => Math.round(value * 1e6);
        return {
            name,
            points: line!.coordinates.map(([lon, lat], i) => {
                const ele = line!.elevation[i];
                return ele === null ? { lat: micro(lat), lon: micro(lon) } : { lat: micro(lat), lon: micro(lon), ele };
            }),
            waypoints: facts!.stops.map(({ point }) => ({ lat: micro(point.coordinate[1]), lon: micro(point.coordinate[0]), name: point.label, note: point.note })),
        };
    }

    const exports = $derived([
        { label: 'GPX', disabled: !facts, run: () => download(writeGpx(course()), 'application/gpx+xml', name, 'gpx') },
        // The stored time is at the pace of the route.
        { label: 'FIT', disabled: !facts, run: () => download(writeFitCourse({ ...course(), elapsedS: summary?.seconds ?? 0 }) as Uint8Array<ArrayBuffer>, 'application/vnd.ant.fit', name, 'fit') },
        { label: 'Plan file', run: () => void act(async () => downloadPlan(plan!, await versions())) },
    ]);
</script>

<Detail message={record === undefined ? 'Loading…' : record === null ? 'This route is not in the library.' : undefined}
    {name} nameLabel="Route name" onrename={rename} {failure}>
    {#snippet tags()}
        {#if summary && summary.days > 1}<span class="tag">{summary.days} days</span>{/if}
        {#if waiting}<span class="tag">Waits for the device</span>
        {:else if sentHere}<span class="tag">{onDevice ? 'On the device' : 'Changed since it was sent'}</span>{/if}
    {/snippet}
    {#snippet buttons()}
        {#if plan}
            <a class="button primary" href={`${ROUTES.planner}/${id}`}>Open in the planner</a>
            <button type="button" class="button" disabled={!waiting && onDevice} onclick={() => send(!waiting)}>{waiting ? 'Do not send' : 'Send to device'}</button>
            <ExportMenu items={exports} />
        {/if}
    {/snippet}
    {#snippet left()}
        {#snippet remover()}
            <button type="button" class="button danger" onclick={() => deleting = true}>Delete</button>
        {/snippet}
        {#if plan}
            <Facts {summary} trip={plan.trip} {line} {facts} {error} {changed}>
                <button type="button" class="button" onclick={duplicate}>Duplicate</button>
                {@render remover()}
            </Facts>
        {:else}
            <Card>
                <p>This route cannot be read.</p>
                {@render remover()}
            </Card>
        {/if}
    {/snippet}
    {#snippet picture()}
        {#if summary}<Picture {library} {id} {summary} {pins} mark={share} onpoint={next => share = next ?? undefined} />{/if}
    {/snippet}
    {#if plan && summary}
        {#key id}<Rows {summary} trip={plan.trip} {line} {facts} {pins} mark={share} onpoint={next => share = next ?? undefined} />{/key}
    {/if}
    {#if deleting}
        <Window title="Delete this route?" onClose={() => deleting = false}>
            <p>“{name}” and its saved versions go. You cannot undo this.</p>
            <div class="window-buttons">
                <button type="button" class="button" onclick={() => deleting = false}>Cancel</button>
                <button type="button" class="button danger" onclick={remove}>Delete</button>
            </div>
        </Window>
    {/if}
</Detail>
