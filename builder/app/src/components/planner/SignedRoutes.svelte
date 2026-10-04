<script lang="ts">
    import { untrack } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import Segmented from './Segmented.svelte';
    import PlannerSelect from './PlannerSelect.svelte';
    import RouteStats from './RouteStats.svelte';
    import TrailMarker from './TrailMarker.svelte';
    import { gradeBands } from '../../lib/planner/grade-data';
    import { cumulative } from '../../lib/planner/editor';
    import { profileAscent, profileDescent } from '../../lib/planner/profile-data';
    import { routeWebsite } from '../../lib/planner/route-overlays';
    import { RADII, recordLine, type RouteFilter, type RouteFinder } from '../../lib/planner/route-finder.svelte';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import type { SearchPlace } from '../../lib/planner/search/types';
    import type { Coordinate } from '../../lib/planner/editor';
    import type { RoutePlan } from '../../lib/planner/signed-route-plan';
    import type { Bounds, CatalogRecord, RouteShape, RouteSort } from '../../lib/planner/signed-routes';
    import type { BikeType } from '../../lib/planner/riding-profiles';

    let { finder, activity, theme = 'light', current = null, placeName, findPlaces, onClose, onPlan }: {
        finder: RouteFinder;
        activity: BikeType;
        theme?: 'light' | 'dark';
        /** The plan that the view hides, for the line that leads back to it. */
        current?: { title: string; km: number } | null;
        /** The nearest place to a map point, from the loaded map. */
        placeName: (coordinate: Coordinate) => string | undefined;
        /** Places for a typed start. */
        findPlaces: (text: string, signal: AbortSignal) => Promise<SearchPlace[]>;
        /** Back to the normal panel; the filters and the list stay. */
        onClose: () => void;
        onPlan: (route: CatalogRecord, plan: RoutePlan) => void;
    } = $props();

    const graded = $derived(activity === 'hiking' || activity === 'mtb');
    const grade = (index: number) => activity === 'mtb' ? `S${index}` : `T${index + 1}`;
    const gradeList = (from: number, to: number) => {
        const names = Array.from({ length: to - from + 1 }, (_, i) => grade(from + i));
        return names.length > 1 ? `${names.slice(0, -1).join(', ')} or ${names.at(-1)}` : names[0];
    };
    // T1, T2, T3, T4 follow the grade bands of the profile; T5 and T6 take the two steepest climb bands.
    const gradeColors = [5, 7, 8, 10, 0, 1].map(band => gradeBands[band][theme === 'dark' ? 'dark' : 'color']);
    const levels = ['', 'Local', 'Regional', 'National', 'International'];
    const kindWords: Record<CatalogRecord['kind'], string> = { hiking: 'hiking route', foot: 'hiking route', bicycle: 'cycling route', mtb: 'mountain bike route' };
    const activityWord = $derived(activity === 'hiking' ? 'hiking' : activity === 'mtb' ? 'mountain bike' : 'cycling');
    const sizes = [{ key: 'distanceKm', label: 'Distance', unit: 'km' }, { key: 'climbM', label: 'Climb', unit: 'm' }] as const;
    const sorts: { value: RouteSort; label: string }[] = [
        { value: 'nearest', label: 'Nearest first' }, { value: 'shortest', label: 'Shortest first' }, { value: 'longest', label: 'Longest first' },
        { value: 'most-climb', label: 'Most climb first' }, { value: 'least-climb', label: 'Least climb first' },
    ];

    const filters = $derived(finder.filters);
    const start = $derived(finder.start);
    const place = $derived(start?.name ?? 'the start');
    const plural = $derived(filters.shape === 'loop' ? 'loops' : 'routes');
    const count = (n: number) => `${n} ${n === 1 ? plural.slice(0, -1) : plural}`;
    const [low, high] = $derived(filters.hardest);
    const reading = $derived(low === 0 ? `Up to ${grade(high)}.` : high === 3 ? `Must include ${grade(low)} or harder.`
        : low === high ? `Hardest part ${grade(low)}.` : `Must include ${grade(low)} or harder, up to ${grade(high)}.`);
    const readingDetail = $derived(low === 0
        ? `Routes whose hardest part is ${gradeList(0, high)}. A ${activity === 'mtb' ? 'trail' : 'path'} without a grade counts as ${grade(0)}.`
        : `Only routes with a part graded ${gradeList(low, high)}.`);
    const range = ({ from, to }: Bounds, unit: string) => from !== undefined && to !== undefined ? `${from}–${to} ${unit}`
        : from !== undefined ? `${from} ${unit} or more` : to !== undefined ? `up to ${to} ${unit}` : '';
    const hardestText = $derived(low === 0 ? `up to ${grade(high)}` : low === high ? grade(low) : `${grade(low)} or harder`);
    const summary = $derived([place, `${filters.radiusKm} km`, { any: 'Any', loop: 'Loop', 'one-way': 'One way' }[filters.shape],
        range(filters.distanceKm, 'km'), range(filters.climbM, 'm'), graded ? hardestText : ''].filter(Boolean).join(' · '));
    const filterNames: Record<RouteFilter, string> = { distanceKm: 'distance', climbM: 'climb', hardest: 'hardest part' };
    const filterText = (filter: RouteFilter) => `${filterNames[filter]} ${filter === 'hardest' ? hardestText : range(filters[filter], filter === 'distanceKm' ? 'km' : 'm')}`;

    // The form folds to its summary once there is a start; Edit opens it again.
    let editing = $state(false);
    const folded = $derived(!!start && !editing);
    let typed = $state('');
    let typedPlaces = $state.raw<SearchPlace[]>([]);
    $effect(() => {
        const text = typed.trim();
        if (start || !text) { typedPlaces = []; return; }
        const abort = new AbortController();
        const timer = setTimeout(() => findPlaces(text, abort.signal).then(places => typedPlaces = places.slice(0, 6), () => {}), 300);
        return () => { clearTimeout(timer); abort.abort(); };
    });
    function clearStart() {
        finder.start = null;
        typed = '';
        void finder.select(null);
    }

    // Searches again when the start, a filter or the activity changes. The search itself writes state that it must not track.
    $effect(() => { void [start, $state.snapshot(filters), activity]; untrack(() => finder.search(activity)); });

    const listed = $derived(finder.matches.slice(0, finder.shown));
    const detail = $derived(finder.detail);
    const plan = $derived(finder.plan);
    const km = (metres: number) => `${(metres / 1000).toFixed(1)} km`;
    // The routed plan gives all four figures once it is ready, so the time never belongs to other figures.
    function figures(route: CatalogRecord) {
        const line = finder.preview;
        if (!line) return { distance: route.length_m / 1000, ascent: route.ascent_m, descent: route.descent_m, hours: null };
        const known = line.elevation.every(height => height !== null);
        return { distance: cumulative(line.coordinates).at(-1)!, ascent: known ? profileAscent(0, 1, line) : null,
            descent: known ? profileDescent(0, 1, line) : null, hours: line.seconds / 3600 };
    }

    function kindLine(route: CatalogRecord, short = false): string {
        if (route.stages) return short ? `${route.stages.length} stages` : `Long route · ${route.stages.length} stages`;
        const total = route.parent ? finder.record(route.parent)?.stages?.length : undefined;
        if (route.stage) return total ? `Stage ${route.stage} of ${total}` : `Stage ${route.stage}`;
        return short ? route.loop ? 'Loop' : 'One way' : route.loop ? 'Signed loop' : 'Signed route';
    }
    // Where the plan starts and ends, by the nearest places of the loaded map.
    const ends = $derived.by(() => {
        const line = detail ? recordLine(detail.route, detail.stages) : [];
        if (!detail || !line.length) return '';
        const [from, to] = [placeName(line[0]), placeName(line.at(-1)!)];
        return detail.route.loop ? from ? `Starts and ends at ${from}` : '' : from && to ? `${from} → ${to}` : '';
    });
    const network = (route: CatalogRecord) => levels[route.rank] ? `${levels[route.rank]} ${kindWords[route.kind]}` : `${kindWords[route.kind][0].toUpperCase()}${kindWords[route.kind].slice(1)}`;
    const rowLine = (route: CatalogRecord) => [kindLine(route, true), levels[route.rank], graded ? `Hardest ${grade(route.hardest ?? 0)}` : ''].filter(Boolean).join(' · ');
    const title = (route: CatalogRecord) => route.name ?? route.ref ?? 'Unnamed route';

    function bound(key: 'distanceKm' | 'climbM', end: 'from' | 'to', value: string) {
        const number = value.trim() === '' ? undefined : Number(value);
        finder.filters[key] = { ...finder.filters[key], [end]: number !== undefined && Number.isFinite(number) && number >= 0 ? number : undefined };
    }
    function hardest(end: 0 | 1, value: number) {
        const next: [number, number] = [...finder.filters.hardest];
        next[end] = value;
        if (next[0] > next[1]) next[1 - end] = value;
        finder.filters.hardest = next;
    }

    let scroller: HTMLDivElement | undefined = $state();
    // A hovered line or marker on the map brings its row into view.
    $effect(() => {
        const id = finder.hovered;
        if (id !== null) scroller?.querySelector(`[data-route="${id}"]`)?.scrollIntoView({ block: 'nearest' });
    });
</script>

<section class="routes-view" aria-label="Signed routes">
    <header class="view-head">
        <button type="button" class="back" aria-label="Back to the planner" onclick={onClose}><Icon name="back" size={18} /></button>
        <h2>Signed routes</h2>
    </header>
    {#if current}<p class="current">Your plan: {current.title}, {current.km.toFixed(1)} km · <button type="button" onclick={onClose}>Show</button></p>{/if}
    {#if detail}
        {@const route = detail.route}
        {@const website = routeWebsite(route.website)}
        <div class="pane-scroll detail">
            <button type="button" class="back-line" onclick={() => finder.select(null)}><Icon name="back" size={15} />{count(finder.matches.length)}</button>
            <div class="head">
                <TrailMarker symbol={route.symbol} ref={route.ref} />
                <h3>{title(route)}<small>{[kindLine(route), detail.distanceM === undefined ? '' : `${km(detail.distanceM)} from ${place}`].filter(Boolean).join(' · ')}</small></h3>
            </div>
            <p class="kind">{route.stage && detail.family ? `Official stage ${route.stage} of ${detail.family.stages!.length} · ${title(detail.family)}, ${km(detail.family.length_m)} · ` : ''}{network(route)}</p>
            <RouteStats {...figures(route)} walking={activity === 'hiking'} />
            {#if ends}<p class="ends"><Icon name="flag" size={16} />{ends}</p>{/if}
            {#if detail.stages && detail.family}
                <p class="sub">{title(detail.family)} stages</p>
                <ol class="stages">
                    {#each detail.stages as stage, i (stage.id)}
                        <li><button type="button" class:here={stage.id === route.id} aria-current={stage.id === route.id} onclick={() => finder.select(stage.id)}>
                            <span class="no">{i + 1}</span><span>{title(stage)}{stage.id === route.id ? ' · this stage' : ''}</span><span class="fig">{km(stage.length_m)}</span>
                        </button></li>
                    {/each}
                    {#if route.stage}
                        <li><button type="button" onclick={() => finder.select(detail.family!.id)}>
                            <span class="no"><Icon name="diamond" size={13} /></span><span>Whole {title(detail.family)} · {km(detail.family.length_m)} · {detail.stages.length} stages</span><Icon name="chevron" size={13} />
                        </button></li>
                    {/if}
                </ol>
            {/if}
            {#if graded && route.grades_m}
                {@const parts = route.grades_m.flatMap((metres, i) => metres > 0 ? [{ i, metres }] : [])}
                <div class="mix">
                    <p class="mix-head"><strong>Grade mix</strong><span>Hardest part {grade(route.hardest ?? 0)}</span></p>
                    <div class="track">{#each parts as { i, metres } (i)}<i style:flex={metres} style:background={gradeColors[i]}></i>{/each}</div>
                    <p class="legend">{#each parts as { i, metres } (i)}<span><i style:background={gradeColors[i]}></i>{grade(i)} {km(metres)}</span>{/each}</p>
                </div>
            {/if}
            {#if route.description}<p class="description">{route.description}</p>{/if}
            {#if route.operator}<p class="sub">Operator</p><p class="operator">{route.operator}</p>{/if}
            {#if website}<p class="sub">Website</p><p class="operator"><a href={website} target="_blank" rel="noreferrer">{new URL(website).hostname.replace(/^www\./, '')}</a></p>{/if}
        </div>
        <footer class="plan-foot">
            {#if plan === null}
                <p class="note">This route is too long for one plan. Choose a stage to plan it.</p>
            {:else if !plan && detail.failed}
                <p class="note" role="alert">This route could not load. Choose a stage to plan it.</p>
            {:else}
                <button type="button" class="primary" disabled={!plan} onclick={() => plan && onPlan(route, plan)}>{plan ? 'Plan this route' : 'Loading the stages…'}<Icon name="arrow" size={15} /></button>
            {/if}
        </footer>
    {:else}
        <div class="pane-scroll" bind:this={scroller}>
            {#if folded}
                <div class="summary"><span>{summary}</span><button type="button" onclick={() => editing = true}>Edit</button></div>
            {:else}
            <div class="form">
                <div class="field"><span class="label">Start</span>
                    {#if start}
                        <div class="start">
                            <Icon name="pin" size={16} />
                            <span>{start.name ?? `Point on the map${start.near ? ` · near ${start.near}` : ''}`}</span>
                            <button type="button" aria-label="Clear the start" onclick={clearStart}><Icon name="close" size={16} /></button>
                        </div>
                    {:else}
                        <!-- svelte-ignore a11y_autofocus -->
                        <label class="start"><Icon name="search" size={16} /><input aria-label="Start place" placeholder="Type a place" autofocus bind:value={typed} /></label>
                        {#each typedPlaces as found (found.source)}
                            <button type="button" class="found" onclick={() => finder.start = { coordinate: [found.lon, found.lat], name: found.name }}>
                                <strong>{found.name}</strong><small>{kindLabel(found.kind)}{found.city && found.city !== found.name ? ` · ${found.city}` : ''}</small>
                            </button>
                        {/each}
                    {/if}
                </div>
                <p class="hint"><Icon name="locate" size={13} />{start ? 'Click the map to move the start.' : 'Type a place, or click the map to move the start.'}</p>
                <div class="two">
                    <div class="field"><span class="label">Within</span>
                        <PlannerSelect label="Within" value={String(filters.radiusKm)} options={RADII.map(radius => ({ value: String(radius), label: `${radius} km` }))} onChange={value => finder.filters.radiusKm = Number(value)} />
                    </div>
                    <div class="field"><span class="label">Shape</span>
                        <Segmented label="Shape" value={filters.shape} onChange={(shape: RouteShape) => finder.filters.shape = shape}
                            options={[{ value: 'any', label: 'Any' }, { value: 'loop', label: 'Loop' }, { value: 'one-way', label: 'One way' }]} />
                    </div>
                </div>
                <div class="two">
                    {#each sizes as { key, label, unit } (key)}
                        <div class="field"><span class="label">{label}</span>
                            <span class="pair">
                                <input type="number" inputmode="numeric" min="0" placeholder="from" aria-label={`${label} from, ${unit}`} value={filters[key].from ?? ''} oninput={event => bound(key, 'from', event.currentTarget.value)} />
                                <i>–</i>
                                <input type="number" inputmode="numeric" min="0" placeholder="to" aria-label={`${label} to, ${unit}`} value={filters[key].to ?? ''} oninput={event => bound(key, 'to', event.currentTarget.value)} />
                                <i>{unit}</i>
                            </span>
                        </div>
                    {/each}
                </div>
                {#if graded}
                    <div class="field"><span class="label">Hardest part</span>
                        <div class="range">
                            <div class="steps" aria-hidden="true">{#each [0, 1, 2, 3] as step (step)}<span class:on={step >= low && step <= high}>{grade(step)}</span>{/each}</div>
                            <div class="rail" style:--low={low} style:--high={high}>
                                <input type="range" min="0" max="3" step="1" value={low} aria-label="Hardest part, easiest grade" aria-valuetext={grade(low)} oninput={event => hardest(0, Number(event.currentTarget.value))} />
                                <input type="range" min="0" max="3" step="1" value={high} aria-label="Hardest part, hardest grade" aria-valuetext={grade(high)} oninput={event => hardest(1, Number(event.currentTarget.value))} />
                            </div>
                        </div>
                    </div>
                    <p class="reading"><b>{reading}</b> {readingDetail}</p>
                {/if}
            </div>
            {/if}
            <div class="results" aria-busy={finder.status === 'loading'}>
                {#if !start}
                    <p class="note">The list shows the routes around the start.</p>
                {:else if finder.status === 'failed'}
                    <p class="note" role="alert">The routes could not load.</p>
                    <button type="button" class="more" onclick={() => finder.search(activity)}>Retry</button>
                {:else if finder.status === 'loading' && finder.progress.loaded < finder.progress.total}
                    <p class="note" role="status">Loading the routes around {place} · {finder.progress.loaded} of {finder.progress.total} map cells</p>
                {:else if !finder.matches.length && finder.blocker}
                    {@const blocker = finder.blocker}
                    <p class="note empty" role="status">No routes match the {filterText(blocker)}.</p>
                    <button type="button" class="more" onclick={() => finder.clear(blocker)}>Clear {filterNames[blocker]}</button>
                {:else if !finder.matches.length}
                    <p class="note empty" role="status">No signed {activityWord} {plural} within {filters.radiusKm} km of {place}.{finder.wider ? ` ${finder.wider.count} within ${finder.wider.radiusKm} km.` : ''}</p>
                    {#if finder.wider}{@const wider = finder.wider.radiusKm}<button type="button" class="more" onclick={() => finder.filters.radiusKm = wider}>Search within {wider} km</button>{/if}
                {:else}
                    <div class="list-head">
                        <strong>{count(finder.matches.length)} within {filters.radiusKm} km</strong>
                        <label class="sort"><span class="visually-hidden">Sort</span>
                            <select value={filters.sort} onchange={event => finder.filters.sort = event.currentTarget.value as RouteSort}>{#each sorts as sort (sort.value)}<option value={sort.value}>{sort.label}</option>{/each}</select>
                            <Icon name="down" size={12} />
                        </label>
                    </div>
                    {#if finder.offline}<p class="note">Only routes inside your download are shown.</p>{/if}
                    {#each listed as { route, distanceM }, i (route.id)}
                        <button type="button" class="row" class:hovered={finder.hovered === route.id} data-route={route.id}
                            onmouseenter={() => finder.hovered = route.id} onmouseleave={() => finder.hovered = null}
                            onfocus={() => finder.hovered = route.id} onblur={() => finder.hovered = null} onclick={() => finder.select(route.id)}>
                            <span class="n">{i + 1}</span>
                            <span class="marker"><TrailMarker symbol={route.symbol} ref={route.ref} /></span>
                            <span class="body"><strong>{title(route)}</strong><small>{rowLine(route)}</small><span class="detail-line">{km(distanceM)} from {place}</span></span>
                            <span class="figure">{km(route.length_m)}<small>↑ {route.ascent_m} m</small></span>
                            <Icon name="chevron" size={13} />
                        </button>
                    {/each}
                    {#if finder.matches.length > finder.shown}
                        <button type="button" class="more" onclick={() => finder.shown += 20}>Show 20 more</button>
                    {/if}
                {/if}
                <p class="attribution">© <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">OpenStreetMap contributors</a> · ODbL</p>
            </div>
        </div>
    {/if}
</section>

<style>
    button { padding: 0; border: 0; background: none; color: inherit; font: inherit; cursor: pointer; }
    button:focus-visible { outline: 2px solid var(--forest); outline-offset: 2px; }
    .routes-view { display: flex; flex-direction: column; flex: 1; min-height: 0; color: var(--ink); }
    .view-head { display: flex; align-items: center; gap: 8px; padding: 14px 16px 6px; }
    .view-head h2 { margin: 0; font: 600 17px var(--sans); }
    .back { display: grid; place-items: center; width: 30px; height: 30px; margin-left: -6px; border-radius: 6px; color: var(--ink-soft); }
    .back:hover, .back-line:hover { background: var(--parchment-2); }
    .pane-scroll { flex: 1; min-height: 0; overflow: auto; }
    .form { display: flex; flex-direction: column; gap: 12px; padding: 8px 16px 4px; }
    .field { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
    .label { font-size: 12px; font-weight: 500; color: var(--ink-soft); }
    .two { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1.5fr); gap: 12px; }
    .two + .two { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .start, .pair input { min-height: 40px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--panel); color: var(--ink); font: 400 14px var(--sans); }
    .start { display: flex; align-items: center; gap: 8px; padding: 0 4px 0 10px; color: var(--ink-soft); }
    .start span { flex: 1; min-width: 0; overflow: hidden; color: var(--ink); text-overflow: ellipsis; white-space: nowrap; }
    .start button { display: grid; place-items: center; width: 28px; height: 32px; color: var(--ink); }
    .hint { display: flex; align-items: center; gap: 6px; margin: -6px 0 0; font-size: 12px; color: var(--ink-soft); }
    .pair { display: flex; align-items: center; gap: 4px; }
    .pair input { width: 0; flex: 1; min-width: 0; padding: 0 8px; font-variant-numeric: tabular-nums; appearance: textfield; -moz-appearance: textfield; }
    .pair input::-webkit-inner-spin-button { -webkit-appearance: none; }
    .pair input::placeholder { color: var(--ink-soft); }
    .pair i { font-style: normal; font-size: 12px; color: var(--ink-soft); }
    .range { padding: 2px 4px 0; }
    .steps { display: grid; grid-template-columns: repeat(4, 1fr); margin-bottom: 4px; font: 600 12px var(--mono); color: var(--ink-faint); text-align: center; }
    .steps .on { color: var(--ink); }
    /* Two range inputs share one rail; only their thumbs take the pointer. */
    .rail { position: relative; height: 24px; margin: 0 12.5%; }
    .rail::before, .rail::after { content: ""; position: absolute; top: 10px; height: 4px; border-radius: 2px; }
    .rail::before { left: 0; right: 0; background: var(--parchment-2); }
    .rail::after { left: calc(var(--low) / 3 * 100%); right: calc((3 - var(--high)) / 3 * 100%); background: var(--ink-soft); }
    .rail input { position: absolute; inset: 0 -10px; width: calc(100% + 20px); height: 24px; margin: 0; background: none; pointer-events: none; appearance: none; -webkit-appearance: none; z-index: 1; }
    .rail input::-webkit-slider-thumb { width: 20px; height: 20px; border: 2px solid var(--ink); border-radius: 50%; background: var(--panel); pointer-events: auto; cursor: pointer; -webkit-appearance: none; }
    .rail input::-moz-range-thumb { width: 16px; height: 16px; border: 2px solid var(--ink); border-radius: 50%; background: var(--panel); pointer-events: auto; cursor: pointer; }
    .rail input:focus-visible { outline: none; }
    .rail input:focus-visible::-webkit-slider-thumb { outline: 2px solid var(--forest); outline-offset: 2px; }
    .reading { margin: 0; padding: 10px 12px; border-radius: 8px; background: var(--parchment); font-size: 13px; line-height: 1.45; }
    .reading b { font-weight: 600; }
    .summary { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 4px 16px 8px; font-size: 13px; }
    .summary span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .summary button, .current button { color: var(--link); text-decoration: underline; text-underline-offset: 3px; }
    .current { margin: 0 16px 6px; font-size: 13px; color: var(--ink-soft); }
    .start input { flex: 1; min-width: 0; border: 0; outline: none; background: transparent; color: var(--ink); font: inherit; }
    .found { display: block; width: 100%; padding: 6px 8px; border-radius: 6px; text-align: left; }
    .found:hover { background: var(--parchment-2); }
    .found strong, .found small { display: block; }
    .found small { color: var(--ink-soft); font-size: 12px; }
    .ends { display: flex; align-items: center; gap: 8px; margin: 0 0 4px; font-size: 13px; }
    .ends :global(svg) { flex: none; color: var(--ink-soft); }
    .results { padding: 12px 16px 16px; }
    .list-head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding-bottom: 8px; border-bottom: 1px solid var(--line); background: var(--panel); font-size: 13px; }
    .list-head strong { font-weight: 600; }
    .sort { position: relative; display: inline-flex; align-items: center; gap: 4px; color: var(--ink-soft); font-size: 12px; }
    .sort select { padding: 0 14px 0 0; border: 0; background: transparent; color: inherit; font: inherit; appearance: none; cursor: pointer; }
    .sort :global(svg) { position: absolute; right: 0; pointer-events: none; }
    .visually-hidden { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
    .row { display: flex; align-items: flex-start; gap: 10px; width: calc(100% + 16px); margin: 0 -8px; padding: 12px 8px; border-radius: 8px; text-align: left; }
    .row:hover, .row.hovered { background: var(--parchment-2); }
    .row .n { flex: none; width: 14px; padding-top: 2px; font: 600 12px var(--mono); color: var(--ink-faint); text-align: right; }
    .row .marker { flex: none; display: flex; min-width: 24px; }
    .row .body { flex: 1; min-width: 0; }
    .row strong { display: block; font: 600 14px/1.4 var(--sans); overflow-wrap: anywhere; }
    .row small { display: block; margin-top: 2px; font: 400 13px/1.4 var(--sans); color: var(--ink-soft); }
    .row .detail-line { display: block; margin-top: 4px; font: 400 12px/1.5 var(--sans); color: var(--ink-soft); }
    .row .figure { flex: none; text-align: right; font: 600 14px var(--sans); font-variant-numeric: tabular-nums; white-space: nowrap; }
    .row > :global(svg) { flex: none; margin-top: 7px; color: var(--ink-soft); }
    .more { min-height: 36px; margin-top: 8px; padding: 7px 10px; border: 1px solid var(--line); border-radius: 6px; font-size: 13px; }
    .more:hover { border-color: var(--ink-soft); }
    .note { margin: 4px 0 10px; font-size: 13px; line-height: 1.45; color: var(--ink-soft); }
    .note.empty { color: var(--ink); }
    .attribution { margin: 16px 0 0; font-size: 11px; color: var(--ink-soft); }
    .attribution a, .operator a { color: var(--link); text-underline-offset: 3px; }
    .detail { padding: 0 16px 16px; }
    .back-line { display: inline-flex; align-items: center; gap: 6px; min-height: 32px; margin: 0 0 8px -4px; padding: 0 6px 0 4px; border-radius: 6px; color: var(--ink-soft); font-size: 13px; }
    .head { display: flex; gap: 12px; align-items: flex-start; }
    .head h3 { margin: 0; font: 600 17px/1.3 var(--sans); overflow-wrap: anywhere; }
    .head small { display: block; margin-top: 2px; font: 400 14px var(--sans); color: var(--ink-soft); }
    .kind { margin: 6px 0 0; font-size: 13px; line-height: 1.45; color: var(--ink-soft); }
    .sub { margin: 14px 0 0; font-size: 12px; font-weight: 600; color: var(--ink-soft); }
    .stages { margin: 6px 0 0; padding: 0; list-style: none; }
    .stages li + li { border-top: 1px solid var(--line); }
    .stages button { display: flex; align-items: baseline; gap: 8px; width: 100%; padding: 6px 0; text-align: left; font-size: 13px; color: var(--ink-soft); }
    .stages button:hover { color: var(--ink); }
    .stages .here { color: var(--ink); font-weight: 600; }
    .stages .no { flex: none; width: 18px; font: 600 12px var(--mono); color: var(--ink-faint); }
    .stages .fig { margin-left: auto; white-space: nowrap; }
    .stages :global(svg) { margin-left: auto; align-self: center; }
    .mix { margin: 12px 0; }
    .mix-head { display: flex; justify-content: space-between; margin: 0 0 6px; font-size: 13px; }
    .mix-head strong { font-weight: 600; }
    .mix-head span { color: var(--ink-soft); font-size: 12px; }
    .track { display: flex; height: 10px; border-radius: 3px; overflow: hidden; background: var(--parchment-2); }
    .legend { display: flex; flex-wrap: wrap; gap: 4px 12px; margin: 6px 0 0; font-size: 12px; color: var(--ink-soft); }
    .legend i { display: inline-block; width: 10px; height: 10px; margin-right: 5px; border-radius: 2px; vertical-align: -1px; }
    .description { margin: 12px 0 0; font-size: 13px; line-height: 1.5; }
    .operator { margin: 4px 0 0; font-size: 13px; overflow-wrap: anywhere; }
    .plan-foot { flex: none; padding: 12px 16px 14px; border-top: 1px solid var(--line); background: var(--panel); }
    .plan-foot .note { margin: 8px 0 0; }
    .primary { display: flex; align-items: center; justify-content: center; gap: 8px; width: 100%; height: 36px; border-radius: 6px; background: var(--amber); color: var(--on-amber); font-weight: 600; }
    .primary:hover:not(:disabled) { filter: brightness(.95); }
</style>
