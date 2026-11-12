<script lang="ts">
    import { tick, untrack } from 'svelte';
    import Card from '../lib/ui/Card.svelte';
    import Window from '../lib/ui/Window.svelte';
    import Icon from '../components/planner/PlannerIcon.svelte';
    import Facts from '../components/route/Facts.svelte';
    import RouteMap from '../components/route/RouteMap.svelte';
    import Cover from '../lib/covers/Cover.svelte';
    import { settled, wantCover } from '../lib/covers/job';
    import Credits from '../components/planner/Credits.svelte';
    import { writeFitCourse } from '../lib/core/fit';
    import { writeGpx, type GpxImport } from '../lib/core/gpx';
    import type { CoverRecord, RouteRecord } from '../lib/library/records';
    import type { Library } from '../lib/library/store';
    import { hasEndpoints, planTitle, planView } from '../lib/planner/editor';
    import { downloadPlan, isPlan, isVersion, newPlan } from '../lib/planner/library';
    import { routeFacts } from '../lib/planner/route-facts';
    import { LegCache } from '../lib/planner/route-legs';
    import { creditsWith } from '../lib/planner/map-setup';
    import { calculateLine, routingKey, type RoutingLine } from '../lib/planner/routing';
    import type { Version } from '../lib/planner/versions';
    import { router } from '../lib/router.svelte';
    import { ROUTES } from '../lib/routes';

    let { library }: { library: Library; active: boolean } = $props();

    const id = $derived(router.id ?? '');
    const legs = new LegCache();
    // Raw: `planView` freezes the trip that it reads.
    let record = $state.raw<RouteRecord | null>();
    let waiting = $state(false), cover = $state.raw<CoverRecord>(), swapped = $state(false);
    let routed = $state.raw<{ key: string; line?: RoutingLine; error?: string }>();
    /** The pointer, as a share of the summary line from 0 to 1. */
    let share = $state<number>();
    let renaming = $state(false), deleting = $state(false), exportOpen = $state(false), failure = $state('');

    const plan = $derived(record && isPlan(record) ? record : undefined);
    const summary = $derived(record?.summary ?? null);
    const name = $derived(plan ? plan.name || planTitle(plan.trip) : record?.name ?? '');
    const key = $derived(plan && hasEndpoints(plan.trip) ? routingKey(plan.trip) : undefined);
    const line = $derived(routed?.key === key ? routed?.line : undefined);
    const error = $derived(!key ? 'This plan has no start and finish.' : routed?.key === key ? routed?.error : undefined);
    const facts = $derived(plan && line ? routeFacts(plan.trip, line) : undefined);
    const changed = $derived(!!plan && !!line && (!summary || Math.abs(planView(plan.trip, line).summary.distance * 1000 - summary.lengthM) > summary.lengthM / 100));
    const covered = $derived(!!cover?.image);
    // The small map draws the basemap too. A cover made before its credits were kept has the credits of the release.
    const credits = $derived(creditsWith(cover?.credits ?? []));
    // A cover takes the large place, unless the rider swapped by hand.
    const mapLarge = $derived(!covered || swapped);

    $effect(() => {
        const target = id;
        let live = true;
        record = undefined;
        [cover, swapped, share, renaming, deleting, exportOpen, failure] = [undefined, false, undefined, false, false, false, ''];
        const load = async () => {
            const [item, ids, stored] = await Promise.all([library.item(target), library.state('waiting'), library.cover(target)]);
            if (!live) return;
            record = item && 'revision' in item ? item : null;
            waiting = !!ids?.includes(target);
            cover = stored;
        };
        // Also the cover of an earlier line, which shows until the new one arrives.
        void load().then(() => { if (live && record?.summary && !settled(record, cover)) wantCover(target, 'page'); });
        const stop = library.onChange((store, changedKey) => {
            if (changedKey === target || (store === 'state' && changedKey === 'waiting')) void load();
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

    async function rename(input: HTMLInputElement) {
        renaming = false;
        const next = input.value.trim().slice(0, 120);
        if (next && next !== record?.name) await act(() => library.renameRoute(id, next));
    }

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

    function download(content: BlobPart, type: string, extension: string) {
        const url = URL.createObjectURL(new Blob([content], { type }));
        const link = document.createElement('a');
        link.href = url;
        link.download = `${name.replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 80) || 'route'}.${extension}`;
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
    }

    function exportAs(kind: 'gpx' | 'fit' | 'plan') {
        exportOpen = false;
        if (kind === 'gpx') download(writeGpx(course()), 'application/gpx+xml', 'gpx');
        // The stored time is at the pace of the route.
        else if (kind === 'fit') download(writeFitCourse({ ...course(), elapsedS: summary?.seconds ?? 0 }) as Uint8Array<ArrayBuffer>, 'application/vnd.ant.fit', 'fit');
        else void act(async () => downloadPlan(plan!, await versions()));
    }

    async function startRename() {
        renaming = true;
        await tick();
        document.querySelector<HTMLInputElement>('.title input')?.select();
    }
</script>

<svelte:window onclick={event => { if (exportOpen && !(event.target as Element).closest('.export')) exportOpen = false; }} />

<div class="route-page">
    {#if record === undefined}
        <p class="message">Loading…</p>
    {:else if record === null}
        <p class="message">This route is not in the library.</p>
        <a class="back" href={ROUTES.routes}>‹ Routes</a>
    {:else}
        <header>
            <a class="back" href={ROUTES.routes}>‹ Routes</a>
            <div class="title">
                {#if renaming}
                    <input aria-label="Route name" value={name}
                        onkeydown={event => { if (event.key === 'Enter') void rename(event.currentTarget); else if (event.key === 'Escape') renaming = false; }}
                        onblur={() => renaming = false} />
                {:else}
                    <h1>{name}</h1>
                    <button type="button" class="icon" aria-label="Rename" onclick={startRename}><Icon name="pencil" size={16} /></button>
                {/if}
                {#if summary && summary.days > 1}<span class="tag">{summary.days} days</span>{/if}
                {#if waiting}<span class="tag">Waits for the device</span>{/if}
            </div>
            {#if plan}
                <nav class="buttons" aria-label="Route">
                    <a class="button primary" href={`${ROUTES.planner}/${id}`}>Open in the planner</a>
                    <button type="button" class="button" onclick={() => send(!waiting)}>{waiting ? 'Do not send' : 'Send to device'}</button>
                    <div class="export">
                        <button type="button" class="button" aria-expanded={exportOpen} onclick={() => exportOpen = !exportOpen}>Export ▾</button>
                        {#if exportOpen}
                            <menu>
                                <li><button type="button" disabled={!facts} onclick={() => exportAs('gpx')}>GPX</button></li>
                                <li><button type="button" disabled={!facts} onclick={() => exportAs('fit')}>FIT</button></li>
                                <li><button type="button" onclick={() => exportAs('plan')}>Plan file</button></li>
                            </menu>
                        {/if}
                    </div>
                </nav>
            {/if}
        </header>
        {#if failure}<p class="failure" role="alert">{failure}</p>{/if}

        {#snippet remover()}
            <button type="button" class="button danger" onclick={() => deleting = true}>Delete</button>
        {/snippet}

        <div class="cards">
            {#if plan}
                <Facts {summary} trip={plan.trip} {facts} {error} {changed}>
                    <button type="button" class="button" onclick={duplicate}>Duplicate</button>
                    {@render remover()}
                </Facts>
            {:else}
                <Card>
                    <p>This route cannot be read.</p>
                    {@render remover()}
                </Card>
            {/if}
            {#if summary}
                <div class="picture" class:map-large={mapLarge}>
                    {#if covered}
                        <div class="slot cover">
                            <Cover {library} {id} mark={share === undefined ? undefined : share * summary.lengthM}
                                onpoint={metres => share = metres === null || !summary.lengthM ? undefined : metres / summary.lengthM} />
                            {#if mapLarge}<button type="button" class="swap" aria-label="Show the picture large" onclick={() => swapped = false}></button>{/if}
                        </div>
                    {/if}
                    {#if covered}<Credits {credits} side="left" />{/if}
                    <div class="slot map">
                        <RouteMap {summary} mark={share} onpoint={next => share = next ?? undefined} />
                        {#if !mapLarge}<button type="button" class="swap" aria-label="Show the map large" onclick={() => swapped = true}></button>{/if}
                    </div>
                </div>
            {/if}
        </div>
    {/if}
</div>

{#if deleting}
    <Window title="Delete this route?" onClose={() => deleting = false}>
        <p>“{name}” and its saved versions go. You cannot undo this.</p>
        <div class="window-buttons">
            <button type="button" class="button" onclick={() => deleting = false}>Cancel</button>
            <button type="button" class="button danger" onclick={remove}>Delete</button>
        </div>
    </Window>
{/if}

<style>
    .route-page {
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
    .title input {
        width: min(480px, 100%);
        padding: 2px 6px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 700 22px/1.2 var(--sans);
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
    .tag {
        flex: none;
        padding: 2px 8px;
        border-radius: 4px;
        background: var(--parchment-2);
        color: var(--ink-soft);
        font-size: 12px;
    }
    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }
    .button {
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
    .button:hover {
        background: var(--parchment-2);
    }
    .primary, .primary:hover {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    .danger {
        border-color: var(--coral);
        color: var(--coral);
    }
    .export {
        position: relative;
    }
    menu {
        position: absolute;
        z-index: 5;
        top: calc(100% + 4px);
        right: 0;
        min-width: 140px;
        margin: 0;
        padding: 4px;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        box-shadow: 0 8px 24px rgb(28 27 20 / 0.18);
        list-style: none;
    }
    menu button {
        width: 100%;
        padding: 7px 10px;
        border: 0;
        border-radius: 5px;
        background: none;
        color: var(--ink);
        text-align: left;
    }
    menu button:not(:disabled):hover {
        background: var(--parchment-2);
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
        min-height: 0;
    }
    .picture {
        position: relative;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--parchment);
    }
    /* The large place fills the picture; the small place is a corner of it. */
    .slot {
        position: absolute;
        inset: 0;
    }
    .picture:not(.map-large) .map, .map-large .cover {
        inset: auto 12px 12px auto;
        z-index: 2;
        width: 180px;
        height: 110px;
        overflow: hidden;
        border: 3px solid var(--panel);
        border-radius: 8px;
        box-shadow: 0 4px 14px rgb(28 27 20 / 0.25);
    }
    /* The attribution shows with the map in the large place. */
    .picture:not(.map-large) .map :global(.maplibregl-ctrl-attrib) {
        display: none;
    }
    .swap {
        position: absolute;
        z-index: 3;
        inset: 0;
        padding: 0;
        border: 0;
        background: none;
    }
    .window-buttons {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
    }
    @media (max-width: 760px) {
        .route-page {
            height: auto;
            padding: 12px 16px;
        }
        h1 {
            font-size: 21px;
            white-space: normal;
        }
        .cards {
            grid-template-columns: 1fr;
            grid-template-rows: auto;
        }
        .picture {
            order: -1;
            aspect-ratio: 3 / 2;
        }
        .picture:not(.map-large) .map, .map-large .cover {
            width: 120px;
            height: 80px;
        }
        menu {
            right: auto;
            left: 0;
        }
    }
</style>
