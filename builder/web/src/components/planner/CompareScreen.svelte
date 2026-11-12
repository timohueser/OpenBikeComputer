<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import PlannerMap from './PlannerMap.svelte';
    import Icon from './PlannerIcon.svelte';
    import Select from './PlannerSelect.svelte';
    import { daySegments, duration, planView, type Trip } from '../../lib/planner/editor';
    import { coordinateAt } from '../../lib/planner/geo';
    import { dayColor } from '../../lib/planner/day-colors';
    import { compareDays, compareSummary } from '../../lib/planner/compare-days';
    import { heightRange, profileHeightAt, profilePaths, profileSamples } from '../../lib/planner/profile-data';
    import { StopFinder, stretchKey } from '../../lib/planner/night-stops.svelte';
    import { nightWindows, proposeNights, withProposals } from '../../lib/planner/stops';
    import { searchPlan } from '../../lib/planner/search/plan';
    import { config } from '../../lib/planner/map-data';
    import { routingKey, type RoutingLine } from '../../lib/planner/routing';
    import { versionTitle, type Version } from '../../lib/planner/versions';
    import type { MapPoint } from '../../lib/planner/map-types';
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';

    let { name, trip, line, routeError, openId, versions, versionLines, theme, onBack, onOpen, onFindLines }: {
        /** The name of the plan. */
        name: string;
        /** The open plan as it is now, and its line: the open version shows them. */
        trip: Trip;
        line?: RoutingLine;
        /** Why the line of the open plan could not be calculated. */
        routeError?: string;
        openId?: string;
        /** The saved versions that are valid plans. */
        versions: Version[];
        /** The lines of the saved versions by routing key, or the message of a failed calculation. */
        versionLines: Record<string, RoutingLine | string>;
        theme: 'light' | 'dark';
        onBack: () => void;
        onOpen: (id: string, name: string) => void;
        /** Calculates the lines of the versions that have none; with `retry`, also the failed ones and a failed open plan. */
        onFindLines: (retry?: boolean) => void;
    } = $props();

    const tones = ['var(--version-a)', 'var(--version-b)'];
    const grey = 'var(--ink-faint)';

    // The rider's picks; a pick that is no longer a version falls back to the open version and the next one.
    let picked = $state<(string | undefined)[]>([]);
    const pair = $derived.by(() => {
        const valid = (id?: string) => versions.some(v => v.id === id) ? id : undefined;
        const first = valid(picked[0]) ?? (versions.find(v => v.id === openId) ?? versions[0])?.id;
        const second = valid(picked[1]) !== first && valid(picked[1]) || versions.find(v => v.id !== first)?.id;
        return [first, second].flatMap(id => versions.filter(v => v.id === id));
    });
    const options = $derived(versions.map(v => ({ value: v.id, label: versionTitle(v) })));

    /** Picking the version of the other picker swaps the two. */
    function pick(index: number, id: string) {
        const ids = pair.map(v => v.id);
        picked = ids[1 - index] === id ? [ids[1], ids[0]] : index ? [ids[0], id] : [id, ids[1]];
    }

    const sides = $derived(pair.map(version => {
        const open = version.id === openId, plan = open ? trip : version.trip, known = open ? line : versionLines[routingKey(plan)];
        return { id: version.id, title: versionTitle(version), open, trip: plan, line: typeof known === 'object' ? known : undefined,
            error: typeof known === 'string' ? known : open && !line ? routeError || undefined : undefined };
    }));
    $effect(() => { void [versions, versionLines]; untrack(() => onFindLines()); });

    // The suggested nights of each version, from a search for the towns around each night as the planner does it. One
    // finder for each version, so a swap of the pickers searches nothing again.
    const known = new Map<string, StopFinder>();
    const finders = $derived(sides.map(side => known.get(side.id) ?? known.set(side.id, new StopFinder()).get(side.id)!));
    const windows = $derived(sides.map(side => nightWindows(side.trip, side.line)));
    const keys = $derived(sides.map((side, i) => Object.values(windows[i]).map(window => stretchKey(routingKey(side.trip), window))));
    $effect(() => sides.forEach((side, i) => {
        if (!side.line) return;
        const route = routingKey(side.trip), stretches = Object.values(windows[i]), finder = finders[i];
        const context = { view: config.bounds, startDate: side.trip.startDate, plan: searchPlan(side.trip, planView(side.trip, side.line)) };
        untrack(() => finder.want(route, context, stretches));
    }));
    const failed = $derived(sides.map((_, i) => keys[i].filter(key => finders[i].found[key]?.state === 'error')));
    const shown = $derived(sides.map((side, i) => {
        if (!side.line) return null;
        const route = routingKey(side.trip);
        const proposals = proposeNights(side.trip, side.line, night => {
            const found = windows[i][night] && finders[i].found[stretchKey(route, windows[i][night])];
            return found && found.state === 'ready' ? found.stops : [];
        });
        const view = planView(withProposals(side.trip, side.line, proposals), side.line);
        return { view, ...compareDays(side.trip, view, proposals), samples: profileSamples(side.line) };
    }));
    const rows = $derived(Array.from({ length: Math.max(0, ...shown.map(s => s?.days.length ?? 0)) }, (_, i) => i));
    const longestDay = $derived(Math.max(0, ...shown.flatMap(s => s?.view.days.map(d => d.hours) ?? [])) || 1);
    const longestRoute = $derived(Math.max(0, ...shown.map(s => s?.view.total ?? 0)) || 1);
    const heights = $derived(heightRange(...shown.map(s => s?.samples ?? [])));

    // The version under the pointer, or the one clicked, and its marked day by index; the pointer wins.
    type Focus = { side: number; day: number | null };
    let pointed = $state<Focus | null>(null);
    let kept = $state<Focus | null>(null);
    const focus = $derived(pointed ?? kept);
    const marked = $derived(focus && focus.day !== null ? shown[focus.side]?.days[focus.day] : undefined);
    let leaving: ReturnType<typeof setTimeout> | undefined;
    function hold(side: number, day: number | null = null) {
        clearTimeout(leaving);
        pointed = { side, day };
    }
    // The pointer crosses the gaps between the parts of one column, so the focus waits before it clears.
    function release() {
        clearTimeout(leaving);
        leaving = setTimeout(() => pointed = null, 150);
    }
    const pointer = (side: number, day: number | null = null) => ({ onpointerenter: () => hold(side, day), onpointerleave: release });
    function keep(side: number, day: number | null = null) {
        kept = kept?.side === side && kept.day === day ? null : { side, day };
    }
    const isMarked = (side: number, riding: number) => focus?.side === side && marked && !marked.rest && marked.day.ridingNumber === riding;

    /** The focused version shows its day colours and the other one grey; without a focus, each has its own colour. */
    function color(side: number, riding: number): string {
        return !focus ? tones[side] : focus.side === side ? dayColor(riding, theme) : grey;
    }
    const tone = (side: number) => focus && focus.side !== side ? grey : tones[side];
    // The map takes colour values, not CSS variables.
    function value(color: string): string {
        return color.startsWith('var(') ? getComputedStyle(document.documentElement).getPropertyValue(color.slice(4, -1)).trim() : color;
    }

    // Version B draws wide and version A narrow on top of it, so a road that both use shows both colours.
    const mapLines = $derived.by(() => { void theme; return [1, 0].flatMap(i => shown[i] ? daySegments(shown[i].view).map(segment => ({
        coordinates: segment.coordinates, color: value(color(i, segment.day)), width: (i ? 9 : 4) + (isMarked(i, segment.day) ? 3 : 0), transfer: segment.leg === 'transfer',
    })) : []); });
    const pins = $derived(shown.flatMap((s, i): MapPoint[] => s ? s.view.itinerary.filter(d => !d.rest && d.ridingNumber < s.view.days.length).map(d => ({
        id: `${i}-night-${d.ridingNumber}`, kind: 'night', coordinate: coordinateAt(s.view.coordinates, d.to), label: `${sides[i].title}: night ${d.number}`,
        markerLabel: String(d.number), color: color(i, d.ridingNumber), fixed: true,
    })) : []));
    const ends = $derived.by(() => {
        const points = sides.flatMap(side => side.line ? [side.trip.points.find(p => p.kind === 'start'), side.trip.points.find(p => p.kind === 'finish')] : []);
        return [...new Map(points.flatMap(p => p ? [[`${p.kind}${p.coordinate}`, { ...p, id: `${p.kind}-${p.coordinate}`, fixed: true } as MapPoint]] : [])).values()];
    });
    // The map fits both routes, again when another version is picked.
    const routes = $derived(sides.flatMap(side => side.line?.coordinates ?? []));
    let map = $state<PlannerMap>();
    const fitKey = $derived(sides.map(side => side.line ? routingKey(side.trip) : '').join());
    $effect(() => { void fitKey; void tick().then(() => untrack(() => map?.fitRoute())); });

    const km = (value: number) => Math.round(value).toLocaleString('en-GB');
    const x = (side: number, progress: number) => progress * (shown[side]?.view.total ?? 0) / longestRoute * 1000;
    const y = (height: number | null) => 105 - ((height ?? heights.low) - heights.low) / (heights.high - heights.low) * 100;
    const uid = $props.id();
    let back = $state<HTMLButtonElement>();
    onMount(() => back?.focus());

    /** What a profile row draws: the riding stretches of each day, each train or bus, the nights, the rest days and the
     * highest ridden point. */
    function profile(side: number) {
        const { view, samples } = shown[side]!, transfers = shown[side]!.transfers.map(({ from, to }) => [from, to]);
        const ridden = (progress: number) => !transfers.some(([from, to]) => progress > from && progress < to);
        const days = view.days.map(day => {
            const cuts = transfers.filter(([from, to]) => from < day.to && to > day.from);
            const ends = [day.from, ...cuts.flat(), day.to];
            return { day, pieces: ends.flatMap((from, i) => i % 2 || i === ends.length - 1 ? [] : [[Math.max(day.from, from), Math.min(day.to, ends[i + 1])]]) };
        });
        const top = samples.filter(s => s.height !== null && ridden(s.progress)).reduce<typeof samples[number] | null>((best, s) => !best || s.height! > best.height! ? s : best, null);
        const rests = [...new Set(view.itinerary.filter(d => d.rest).map(d => view.days[d.ridingNumber - 1].to))];
        return { paths: profilePaths(samples, heights.low, heights.high, view.total / longestRoute), days, top, rests,
            trains: transfers.map(([from, to]) => `M${x(side, from)} ${y(profileHeightAt(from, samples))}L${x(side, to)} ${y(profileHeightAt(to, samples))}`) };
    }
</script>

<section class="compare" aria-label="Compare versions">
    <header class="topbar">
        <div class="title">
            <button type="button" class="planner-action" bind:this={back} onclick={onBack}><Icon name="back" size={15} />Planner</button>
            <h1>{name}</h1>
        </div>
        {#if pair.length === 2}
            <div class="pickers">
                <span>Compare</span>
                <Select label="First version" value={pair[0].id} {options} onChange={id => pick(0, id)} />
                <span>with</span>
                <Select label="Second version" value={pair[1].id} {options} onChange={id => pick(1, id)} />
            </div>
        {/if}
    </header>
    {#if pair.length < 2}
        <p class="empty">Save two versions of this plan to compare them.</p>
    {:else}
    <div class="body">
        <aside class="days" aria-label="Both versions by day">
            <div class="grid heads">
                <span></span>
                {#each sides as side, i (side.id)}
                    {@const s = shown[i]}
                    <div class="head" class:focused={focus?.side === i} class:dim={focus && focus.side !== i} {...pointer(i)}>
                        <div class="name">
                            <span class="dot" style:background={tones[i]}></span><strong>{side.title}</strong>
                            {#if side.open}<span class="open">open</span>{:else}<button type="button" class="planner-action" onclick={() => onOpen(side.id, side.title)}>Open in planner</button>{/if}
                        </div>
                        {#if s}
                            <p>{compareSummary(s.days)}</p>
                            <p class="totals">{km(distanceIn(s.view.summary.distance * 1000))} {settings.units}{side.line?.unknownElevationKm === 0 ? ` · ↑ ${km(heightIn(s.view.summary.ascent))} ${heightUnit()}` : ''} · {duration(s.view.summary.hours)} riding</p>
                            {#if failed[i].length}
                                <p role="alert">Some towns for the nights could not load, so some nights end at even points.</p>
                                <button type="button" class="planner-action" onclick={() => failed[i].forEach(key => finders[i].retry(key))}>Retry</button>
                            {/if}
                        {:else if side.error}
                            <p role="alert">{side.error}</p>
                            <button type="button" class="planner-action" onclick={() => onFindLines(true)}>Retry</button>
                        {:else}
                            <p role="status">Calculating the route…</p>
                        {/if}
                    </div>
                {/each}
            </div>
            <div class="grid rows">
                {#each rows as row (row)}
                    <span class="label">Day {row + 1}</span>
                    {#each sides as side, i (side.id)}
                        {@const cell = shown[i]?.days[row]}
                        {@const dim = focus && focus.side !== i}
                        {#if !shown[i]}
                            <div class="cell"></div>
                        {:else if !cell}
                            <div class="cell finished" class:dim data-side={i} {...pointer(i)}>Finished</div>
                        {:else if cell.rest}
                            <div class="cell rest" class:dim data-side={i} {...pointer(i)}>
                                {#if cell.date}<small>{cell.date}</small>{/if}
                                <strong><Icon name="pause" size={13} />{cell.name}</strong>
                                <span class="stay"><Icon name={cell.stay.icon} size={14} />{cell.stay.label}</span>
                            </div>
                        {:else}
                            {@const riding = cell.day.ridingNumber}
                            <button type="button" class="cell ride" class:dim class:marked={isMarked(i, riding)} data-side={i} aria-pressed={kept?.side === i && kept.day === row}
                                {...pointer(i, row)} onclick={() => keep(i, row)}>
                                {#if cell.date}<small>{cell.date}</small>{/if}
                                <strong>{cell.from} → {cell.to}</strong>
                                <span class="figures">{km(distanceIn(cell.day.distance * 1000))} {settings.units} · {duration(cell.day.hours)}{side.line?.unknownElevationKm === 0 ? ` · ↑ ${km(heightIn(cell.day.ascent))} ${heightUnit()}` : ''}</span>
                                <span class="time" aria-hidden="true"><span style:width={`${cell.day.hours / longestDay * 100}%`} style:background={color(i, riding)}></span></span>
                                {#each cell.trains as train (train)}<span class="stay"><Icon name="train" size={14} />Train or bus {train}</span>{/each}
                                <span class="stay"><Icon name={cell.stay.icon} size={14} />{cell.stay.label}</span>
                            </button>
                        {/if}
                    {/each}
                {/each}
            </div>
        </aside>
        <div class="geography">
            <div class="map-area">
                <PlannerMap bind:this={map} {theme} coordinates={routes} points={[...ends, ...pins]} lines={mapLines} />
                <div class="legend">
                    {#each sides as side, i (side.id)}
                        <button type="button" class="chip" class:focused={focus?.side === i} aria-pressed={kept?.side === i && kept.day === null} {...pointer(i)} onclick={() => keep(i)}>
                            <span class="dot" style:background={tone(i)}></span><strong>{side.title}</strong>
                            {#if shown[i]}<small>{shown[i].days.length} days</small>{/if}
                        </button>
                    {/each}
                    <p>{focus ? `${sides[focus.side].title} in its day colours, ${sides[1 - focus.side].title} in grey.` : 'Point at a version to see its days. Click it to keep it.'}</p>
                </div>
                <div class="zoom">
                    <button type="button" onclick={() => map?.zoomBy(1)} aria-label="Zoom in"><Icon name="plus" /></button>
                    <button type="button" onclick={() => map?.zoomBy(-1)} aria-label="Zoom out"><Icon name="minus" /></button>
                </div>
            </div>
            <section class="profiles" aria-label="Elevation of both versions on one scale">
                {#each sides as side, i (side.id)}
                    {#if shown[i]}
                        {@const drawn = profile(i)}
                        <div class="profile" class:focused={focus?.side === i} {...pointer(i)}>
                            <svg viewBox="0 0 1000 112" preserveAspectRatio="none" aria-hidden="true">
                                {#if marked && focus?.side === i && !marked.rest}<rect class="band" x={x(i, marked.day.from)} width={x(i, marked.day.to) - x(i, marked.day.from)} y="0" height="112" />{/if}
                                {#each drawn.days as { day, pieces } (day.number)}
                                    <clipPath id={`${uid}-${i}-${day.number}`}>{#each pieces as [from, to], k (k)}<rect x={x(i, from)} width={x(i, to) - x(i, from)} y="0" height="112" />{/each}</clipPath>
                                    <g clip-path={`url(#${uid}-${i}-${day.number})`} style:--tone={color(i, day.number)}>
                                        <path d={drawn.paths.area} class="area" />
                                        <path d={drawn.paths.line} class="line" />
                                    </g>
                                {/each}
                                {#each drawn.trains as train (train)}<path d={train} class="train" style:--tone={tone(i)} />{/each}
                            </svg>
                            {#each drawn.days.slice(0, -1) as { day } (day.number)}<span class="night" style:left={`${x(i, day.to) / 10}%`}></span>{/each}
                            {#each drawn.rests as at (at)}<span class="rest-mark" style:left={`${x(i, at) / 10}%`}>Rest</span>{/each}
                            {#if drawn.top}<span class="top" style:left={`${x(i, drawn.top.progress) / 10}%`} style:top={`${y(drawn.top.height) / 112 * 100}%`}>{km(heightIn(drawn.top.height!))} {heightUnit()}</span>{/if}
                            <span class="caption"><span class="dot" style:background={tone(i)}></span><strong>{side.title}</strong> · {km(distanceIn(shown[i].view.summary.distance * 1000))} {settings.units} ridden</span>
                        </div>
                    {/if}
                {/each}
                <div class="axis"><span>0 {settings.units}</span><span>{km(distanceIn(longestRoute * 1000))} {settings.units}</span></div>
            </section>
        </div>
    </div>
    {/if}
</section>

<style>
    .compare { display: flex; flex-direction: column; flex: 1; min-height: 0; background: var(--panel); }
    .topbar { display: flex; align-items: center; gap: 24px; height: 56px; flex: none; padding: 0 16px; border-bottom: 1px solid var(--line); }
    .title { display: flex; align-items: center; gap: 12px; min-width: 0; }
    .title .planner-action { gap: 6px; }
    h1 { margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font: 700 17px var(--sans); }
    .pickers { display: flex; align-items: center; gap: 10px; font-size: 13px; color: var(--ink-soft); }
    .empty { margin: 24px; color: var(--ink-soft); }
    .body { display: grid; grid-template-columns: min(560px, 45%) minmax(0, 1fr); flex: 1; min-height: 0; }
    .days { display: flex; flex-direction: column; gap: 10px; min-height: 0; padding: 16px 20px; border-right: 1px solid var(--line); }
    .grid { display: grid; grid-template-columns: 48px minmax(0, 1fr) minmax(0, 1fr); gap: 6px 12px; }
    .rows { flex: 1; min-height: 0; align-content: start; overflow: auto; padding-top: 10px; border-top: 1px solid var(--line); }
    .head { display: flex; flex-direction: column; gap: 4px; margin: -6px; padding: 6px; border-radius: 8px; }
    .head.focused { background: var(--parchment-2); }
    .head p { margin: 0; font-size: 13px; line-height: 1.45; color: var(--ink-soft); }
    .head .totals { font-weight: 600; color: var(--ink); }
    .name { display: flex; align-items: center; gap: 8px; min-width: 0; }
    .name strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font: 700 16px var(--sans); }
    .name .planner-action { min-height: 26px; padding: 0 8px; font-size: 12px; white-space: nowrap; }
    .open { font-size: 12px; color: var(--ink-soft); }
    .dot { width: 12px; height: 12px; flex: none; border-radius: 50%; }
    .dim { opacity: .45; }
    .label { padding-top: 6px; font: 700 13px var(--sans); }
    .cell { display: flex; flex-direction: column; gap: 2px; min-width: 0; padding: 6px 10px; border: 1px solid var(--line); border-radius: 8px; background: var(--panel); color: var(--ink); text-align: left; font: inherit; }
    .ride { cursor: pointer; }
    .ride:hover, .ride.marked { background: var(--parchment-2); }
    .ride[aria-pressed="true"] { border: 1.5px solid var(--ink); }
    .cell small { font-size: 11px; color: var(--ink-soft); }
    .cell strong { display: flex; align-items: center; gap: 6px; overflow: hidden; font: 600 13px var(--sans); white-space: nowrap; text-overflow: ellipsis; }
    .figures { font-size: 12px; color: var(--ink-soft); font-variant-numeric: tabular-nums; }
    .time { display: block; height: 4px; margin: 2px 0; padding: 0; border: 0; border-radius: 2px; background: var(--parchment-2); }
    .time span { display: block; height: 100%; border-radius: 2px; }
    .stay { display: flex; align-items: center; gap: 5px; min-width: 0; font-size: 12px; }
    .rest { justify-content: center; border-color: transparent; background: var(--parchment-2); }
    .finished { justify-content: center; align-items: center; border: 1.5px dashed var(--line-strong); font-size: 12px; color: var(--ink-soft); }
    .geography { display: flex; flex-direction: column; min-width: 0; min-height: 0; }
    .map-area { position: relative; flex: 1; min-height: 240px; }
    .legend { position: absolute; left: 12px; top: 12px; z-index: 3; display: flex; flex-direction: column; gap: 6px; max-width: 300px; padding: 10px; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); }
    .legend p { margin: 0; font-size: 12px; color: var(--ink-soft); }
    .chip { display: flex; align-items: center; gap: 8px; padding: 6px 10px; border: 1px solid var(--line); border-radius: 6px; background: var(--panel); color: var(--ink); font: inherit; cursor: pointer; }
    .chip.focused { background: var(--parchment-2); }
    .chip[aria-pressed="true"] { border: 1.5px solid var(--ink); }
    .chip strong { font: 600 13px var(--sans); }
    .chip small { margin-left: auto; font-size: 12px; color: var(--ink-soft); }
    .zoom { position: absolute; top: 16px; right: 16px; z-index: 3; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); }
    .zoom button { display: grid; place-items: center; width: 36px; height: 36px; padding: 0; border: 0; background: none; color: var(--ink); cursor: pointer; }
    .zoom button + button { border-top: 1px solid var(--line); }
    .profiles { flex: none; display: flex; flex-direction: column; gap: 6px; padding: 8px 24px 8px 24px; border-top: 1px solid var(--line); }
    .profile { position: relative; height: 84px; }
    svg { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; }
    path { fill: none; vector-effect: non-scaling-stroke; }
    .area { fill: var(--tone); fill-opacity: .25; }
    .line { stroke: var(--tone); stroke-width: 2; }
    .train { stroke: var(--tone); stroke-width: 2; stroke-dasharray: 4 4; }
    .band { fill: var(--ink); fill-opacity: .08; }
    .night { position: absolute; top: 0; bottom: 0; width: 0; border-left: 1px dashed var(--line-strong); }
    .rest-mark, .top, .caption { position: absolute; font-size: 11px; white-space: nowrap; font-variant-numeric: tabular-nums; }
    .rest-mark { top: 2px; padding: 0 4px; border-radius: 4px; background: var(--parchment-2); transform: translateX(-50%); }
    .top { transform: translate(4px, -100%); font-weight: 600; }
    .caption { left: 0; top: 0; display: flex; align-items: center; gap: 6px; color: var(--ink-soft); }
    .caption strong { color: var(--ink); }
    .caption .dot { width: 10px; height: 10px; }
    .axis { display: flex; justify-content: space-between; font-size: 11px; color: var(--ink-faint); font-variant-numeric: tabular-nums; }
</style>
