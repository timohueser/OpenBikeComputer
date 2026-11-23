<script lang="ts">
    import type { ComponentProps, Snippet } from 'svelte';
    import DrawerTitle from './DrawerTitle.svelte';
    import PlanTotals from './PlanTotals.svelte';
    import Surface from './PlannerSurface.svelte';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import Lanes from '../../lib/ui/Lanes.svelte';
    import SelectedPart from '../../lib/ui/SelectedPart.svelte';
    import { formatGrade, gradeBands, gradePaths, profileGrades } from '../../lib/planner/grade-data';
    import { dayColor } from '../../lib/planner/day-colors';
    import { formatPart, heightRange, profileAscent, profileHeightAt, profilePaths, profileSamples, sampleIndex, sampleSlice } from '../../lib/planner/profile-data';
    import type { RoutingLine, Surface as SurfaceKind } from '../../lib/planner/routing';
    import type { Day } from '../../lib/planner/editor';
    import { axisLeft, axisProgress } from '../../lib/planner/profile-axis';
    import { sleepIcon } from '../../lib/planner/search/presentation';
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    type ProfileDay = Pick<Day, 'number' | 'from' | 'to' | 'pinned' | 'split'>;

    let {
        lineData, total, days, walking = false, singleRoute = true, theme = 'light', activeNight, focus = null, window: view = { from: 0, to: 1 }, height = 260, open = true,
        totals = null, onToggle, onNight, onDayEndDrag, onHover, strip, part = null, onPart, marker = null, surface = $bindable(),
        week = null, tripStrip,
    }: {
        /** Route length in km. */
        lineData?: RoutingLine;
        total: number;
        days: ProfileDay[];
        singleRoute?: boolean;
        walking?: boolean;
        theme?: 'light' | 'dark';
        activeNight: number;
        /** The open day. The profile stays on the trip, with the other days quiet, until the rider chooses the day in the
         * range switch. */
        focus?: { from: number; to: number; label: string } | null;
        /** The stretch of the route the map shows, as route progress. */
        window?: { from: number; to: number };
        height?: number;
        /** A closed drawer shows its title only. */
        open?: boolean;
        /** The totals of the whole plan, whatever stretch the profile shows. */
        totals?: ComponentProps<typeof PlanTotals> | null;
        onToggle: () => void;
        onNight: (night: number) => void;
        onDayEndDrag: (night: number, progress: number) => void;
        onHover: (progress: number | null) => void;
        /** A data layer strip for the shown stretch, as route progress, and the hover callback. */
        strip?: Snippet<[number, number, (progress: number | null) => void]>;
        /** The selected part, as route progress. The profile shows the share of it in the shown stretch. */
        part?: [number, number] | null;
        onPart?: (part: [number, number] | null) => void;
        /** The place on the route under the pointer on the map, as route progress. */
        marker?: number | null;
        /** The surface that the rider points at in the surface legend. */
        surface?: SurfaceKind;
        /** The week in focus of a long trip: its days, and its stretch as route progress. The profile then shows
         * the week, and has a range switch only while a day is open. */
        week?: { start: number; end: number; from: number; to: number } | null;
        /** The strip of all days, under the title row while there is a week. */
        tripStrip?: Snippet;
    } = $props();

    // The drawer height counts one title row: the top padding, the row of the range switch and its margin.
    // A wrapped title row takes its height from the map, so the plot keeps its size.
    const bodyHeight = $derived(height - 42);
    const samples = $derived(profileSamples(lineData));
    const known = $derived(samples.flatMap(s => s.height === null ? [] : [s.height]));
    const { low, high } = $derived(heightRange(samples));
    const area = $derived(singleRoute ? profilePaths(samples, low, high).area : '');
    // Each day draws only its own samples; its svg clips them to the day.
    const dayPaths = $derived(days.map(day => profilePaths(sampleSlice(samples, day.from, day.to), low, high)));
    let pointer = $state<number | null>(null);
    let gradeChoice = $state<boolean | null>(null);
    const showGrade = $derived(gradeChoice ?? singleRoute);
    const grades = $derived(profileGrades(samples, total));
    const bandPaths = $derived(gradePaths(samples, grades, low, high));
    function color(index: number) { return theme === 'dark' ? gradeBands[index].dark : gradeBands[index].color; }
    let plot = $state<HTMLDivElement>();
    let drag = $state<{ night: number; progress: number; moved: boolean } | null>(null);
    let range = $state<'map' | 'route'>('map');
    let body = $state<HTMLDivElement>();
    let rows = $state<HTMLDivElement>();
    let content = $state<HTMLDivElement>();
    // More content below the visible part shows as a fade at the bottom edge.
    let more = $state(false);
    function measure() { if (body) more = body.scrollTop + body.clientHeight < body.scrollHeight - 1; }
    $effect(() => {
        if (!body || !content) return;
        const observer = new ResizeObserver(measure);
        observer.observe(body);
        observer.observe(content);
        return () => observer.disconnect();
    });

    // While a part that a drag here started is selected, the profile keeps the stretch of the map from when it started.
    let held = $state<{ from: number; to: number }>();
    $effect.pre(() => { if (!part) held = undefined; });
    // A trial for the owner to compare: with `?day=follow` in the address, the profile shows a day alone when it opens.
    const follow = new URLSearchParams(location.search).get('day') === 'follow';
    // The open day that the rider chose to see alone. Another day, and the same day when it opens again, show the trip.
    let chosen = $state<string>();
    $effect.pre(() => { if (focus?.label !== chosen) chosen = undefined; });
    const alone = $derived(focus && (follow ? range === 'map' : chosen === focus.label) ? focus : null);
    const shown = $derived(alone ?? week ?? (focus || range === 'route' ? { from: 0, to: 1 } : part && held ? held : view));
    const origin = $derived(alone?.from ?? 0);
    const quiet = (day: ProfileDay) => !!focus && day.number !== activeNight;
    const span = $derived(Math.max(1e-6, shown.to - shown.from));
    const shownDays = $derived(days.flatMap((day, i) => day.to > shown.from && day.from < shown.to ? [{ day, paths: dayPaths[i] }] : []));
    // The pointer in the profile, else the pointer on the map while it is in the shown stretch.
    const hover = $derived(pointer ?? (marker !== null && marker >= shown.from && marker <= shown.to ? marker : null));
    const hoverHeight = $derived(hover === null ? null : profileHeightAt(hover, samples));
    const hoverIndex = $derived(hover === null ? -1 : Math.max(0, sampleIndex(samples, hover) - 1));
    const hoverGrade = $derived(grades[hoverIndex] ?? null);
    const hoverPushing = $derived(lineData?.edges.pushing?.[hoverIndex]);
    // Ticks less than 1 unit apart need a decimal, or the labels repeat.
    const ticks = $derived([0, .25, .5, .75, 1].map(t => ({ at: t,
        label: `${distanceIn((shown.from + t * span - origin) * total * 1000).toFixed(distanceIn(span * total * 1000) / 4 < 1 ? 1 : 0)} ${settings.units}` })));
    const endAt = (day: ProfileDay) => drag?.night === day.number ? drag.progress : day.to;
    const share = (progress: number) => Math.min(1, Math.max(0, (progress - shown.from) / span));
    const seen = $derived<[number, number] | null>(part && part[1] > shown.from && part[0] < shown.to ? [Math.max(part[0], shown.from), Math.min(part[1], shown.to)] : null);
    // From the start of the open day, as the ticks and the readout.
    const partText = $derived(seen ? formatPart((seen[0] - origin) * total * 1000, (seen[1] - origin) * total * 1000,
        known.length ? profileAscent(seen[0], seen[1], lineData) : null) : '');
    // The day ends inside the shown stretch; with none inside, the nearest one on each side sits at the edge so it can still be dragged in.
    const handles = $derived.by(() => {
        const ends = days.slice(0, -1).map(day => ({ day, at: endAt(day) }));
        const inside = ends.filter(end => end.at >= shown.from && end.at <= shown.to).map(end => ({ ...end, left: axisLeft(shown, end.at) }));
        if (inside.length) return inside;
        const before = ends.filter(end => end.at < shown.from).at(-1);
        const after = ends.find(end => end.at > shown.to);
        return [...(before ? [{ ...before, left: '0%' }] : []), ...(after ? [{ ...after, left: '100%' }] : [])];
    });

    function progressAt(event: PointerEvent) {
        const box = plot!.getBoundingClientRect();
        return axisProgress(shown, event.clientX - box.left, box.width);
    }

    function track(event: PointerEvent) {
        if (!drag) return;
        const day = days[drag.night - 1];
        drag.progress = Math.max(day.from + .005, Math.min(days[drag.night].to - .005, progressAt(event)));
        drag.moved = true;
        pointer = drag.progress;
        onHover(pointer);
    }

    /** The pointer of `Lanes`; while a day end is dragged, `track` moves it. */
    function point(at: number | null) {
        if (drag) return;
        inspect(at === null ? null : shown.from + at * span);
    }

    function select(next: [number, number] | null) {
        const progress: [number, number] | null = next && [shown.from + next[0] * span, shown.from + next[1] * span];
        if (progress && !part) held = view;
        onPart?.(progress);
    }

    function inspect(progress: number | null) {
        pointer = progress;
        onHover(progress);
    }

    function press(event: PointerEvent, day: ProfileDay) {
        if (event.button !== 0) return;
        event.preventDefault();
        (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
        drag = day.pinned ? null : { night: day.number, progress: day.to, moved: false };
        if (day.pinned) onNight(day.number);
    }

    function release() {
        if (!drag) return;
        const { night, progress, moved } = drag;
        drag = null;
        if (moved) onDayEndDrag(night, progress);
        else onNight(night);
    }
</script>

<section class="elevation" class:open aria-label="Elevation profile">
    <div class="title">
        <DrawerTitle title="Elevation" {open} {onToggle} iconOnly={!!totals} />
        {#if totals}<PlanTotals {...totals} />{/if}
        {#if open}
        {#if !week || focus}<Segmented compact label="Profile range" value={focus ? alone ? 'map' : 'route' : range}
            onChange={(value) => { if (focus && !follow) chosen = value === 'map' ? focus.label : undefined; else range = value; }}
            options={[{ value: 'map', label: focus?.label ?? 'Map view' }, { value: 'route', label: week ? `Days ${week.start}–${week.end}` : 'Whole route' }]} />{/if}
        <label class="grade-toggle"><input type="checkbox" checked={showGrade} onchange={(event) => gradeChoice = event.currentTarget.checked} />Grade</label>
        {#if seen}
            <!-- Clear leaves the document, so the focus goes to the profile. -->
            <div class="selected"><SelectedPart text={partText} onclear={() => { select(null); rows?.querySelector<HTMLElement>('[role="slider"]')?.focus(); }} /></div>
        {/if}
        {/if}
    </div>
    {#if open && week}{@render tripStrip?.()}{/if}
    {#if open}
    <!-- Scrolls when the strips do not fit, so they never cover the line below the drawer. -->
    <div class="body" class:more style:height={`${bodyHeight}px`} bind:this={body} onscroll={measure}><div class="content" bind:this={content}>
    {#if !known.length}<p class="unavailable">Elevation unavailable</p>{/if}
    {#snippet profile()}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="plot" bind:this={plot} onpointermove={track} onpointerup={release} onpointercancel={() => drag = null}>
        <span class="height" style:top="5%">{Math.round(heightIn(high))} {heightUnit()}</span>
        <span class="height" style:top="94%">{Math.round(heightIn(low))} {heightUnit()}</span>
        <svg viewBox={`${shown.from * 1000} 0 ${span * 1000} 112`} preserveAspectRatio="none"
            role="img" aria-label={`${alone?.label ?? 'Route'} elevation profile. Distances start at ${origin > 0 ? 'the day start' : 'the route start'}.`}>
            {#if singleRoute}
                <path d={area} class="terrain-fill" />
            {:else}
                {#each shownDays as { day, paths } (day.number)}
                    <svg x={day.from * 1000} width={Math.max(0, day.to - day.from) * 1000} height="112" viewBox={`${day.from * 1000} 0 ${Math.max(0, day.to - day.from) * 1000} 112`} preserveAspectRatio="none" overflow="hidden">
                        <path d={paths.area} style:--day-color={dayColor(day.number, theme)} class="day-fill" class:quiet={quiet(day)} />
                    </svg>
                {/each}
            {/if}
            <path d="M0 25H1000M0 65H1000M0 105H1000" class="grid" />

            {#if showGrade}
                {#each bandPaths as path, i (i)}
                    <path d={path} style:stroke={color(i)} class="grade-line" />
                {/each}
            {:else}
                {#each shownDays as { day, paths } (day.number)}
                    <svg x={day.from * 1000} width={Math.max(0, day.to - day.from) * 1000} height="112" viewBox={`${day.from * 1000} 0 ${Math.max(0, day.to - day.from) * 1000} 112`} preserveAspectRatio="none" overflow="hidden">
                        <path d={paths.line} fill="none" style:stroke={quiet(day) ? 'var(--quiet)' : dayColor(day.number, theme)} />
                    </svg>
                {/each}
            {/if}
            {#each days.slice(0, -1) as day (day.number)}
                <line x1={day.to * 1000} x2={day.to * 1000} y1="0" y2="112" class:pinned={day.pinned} />
            {/each}
        </svg>
        {#each handles as { day, left } (day.number)}
            <button
                type="button"
                class="handle"
                class:pinned={day.pinned}
                class:moved={day.split}
                class:active={activeNight === day.number}
                style:left={left}
                style:--day-color={dayColor(day.number, theme)}
                title={day.pinned ? 'Pinned · Change overnight in the day' : `${day.split ? 'Day end you moved' : 'Day end suggested'} · drag along the route`}
                aria-label={`Day ${day.number} ends here${day.pinned ? ', pinned' : day.split ? ', moved by you' : ', suggested'}. Show the day.`}
                onpointerdown={(event) => press(event, day)}
                onclick={(event) => { if (event.detail === 0) onNight(day.number); }}
            >{#if day.pinned}<Icon name={sleepIcon(day.pinned.placeKind)} size={13} />{:else}{day.number}{/if}</button>
        {/each}
    </div>
    {/snippet}
    {#snippet readout()}
        {distanceIn(((hover ?? 0) - origin) * total * 1000).toFixed(1)} {settings.units}{origin > 0 ? ` into ${alone!.label.toLowerCase()}` : ''} · {hoverHeight === null ? 'Elevation unknown' : `${Math.round(heightIn(hoverHeight))} ${heightUnit()}`} · {formatGrade(hoverGrade)}{#if hoverPushing && !walking} · Push bike{:else if hoverPushing === null} · Access unverified{/if}
    {/snippet}
    <div class="plot-rows" bind:this={rows}>
        <Lanes label="Profile" rows={[{ label: '', draw: profile, grow: true }]} {ticks} pointer={hover === null ? undefined : share(hover)}
            chip={readout} onpointer={point} part={seen && [share(seen[0]), share(seen[1])]} onpart={onPart && select} onclick={() => { if (seen) select(null); }} />
    </div>
    <Surface {walking} line={lineData} from={shown.from} to={shown.to} bind:lit={surface} onHover={inspect} />
    {@render strip?.(shown.from, shown.to, inspect)}
    </div></div>
    {/if}
</section>

<style>
    .elevation {
        --terrain: color-mix(in srgb, var(--ink-faint) 10%, var(--panel));
        /* The grey of a quiet day on the map. */
        --quiet: color-mix(in srgb, var(--ink-faint) 55%, var(--panel));
        display: flex;
        flex-direction: column;
        /* At a short window, the map holds its minimum height and the body scrolls. */
        flex: 0 1 auto;
        min-height: 0;
        container-type: inline-size;
        padding: 8px 24px;
        background: var(--panel);
    }
    .elevation.open { padding-bottom: 0; }
    .body {
        flex: 0 1 auto;
        min-height: 0;
        overflow-y: auto;
        scrollbar-width: thin;
        scrollbar-color: var(--line-strong) transparent;
    }
    .content {
        position: relative;
        display: flex;
        flex-direction: column;
        min-height: 100%;
        padding-bottom: 8px;
    }
    .body.more::after {
        content: "";
        position: sticky;
        bottom: 0;
        display: block;
        height: 20px;
        margin-top: -20px;
        background: linear-gradient(color-mix(in srgb, var(--panel) 0%, transparent), var(--panel));
        pointer-events: none;
    }
    .title {
        display: flex;
        flex: none;
        align-items: center;
        gap: 12px;
        flex-wrap: wrap;
        min-height: 28px;
        font-size: 13px;
    }
    .open .title { margin-bottom: 4px; }
    .grade-toggle { display: flex; align-items: center; gap: 6px; margin-left: 4px; min-height: 28px; font-weight: 600; cursor: pointer; }
    .grade-toggle input { width: 14px; height: 14px; margin: 0; accent-color: var(--ink-soft); }
    .grade-toggle input:focus-visible { outline: 2px solid var(--ink); outline-offset: 3px; }
    .unavailable { margin: 4px 0 8px 48px; color: var(--ink-soft); font-size: 12px; }
    .selected { margin-left: auto; }
    .plot-rows {
        display: flex;
        flex: 1;
        flex-direction: column;
        /* The strips scroll before the profile gets too flat to read: 72 px of plot over the ticks. */
        min-height: 94px;
        margin: 0 12px 0 48px;
    }
    .plot {
        position: relative;
        height: 100%;
    }
    .plot > svg {
        /* Out of the flow: the viewBox ratio would otherwise set the plot height. */
        position: absolute;
        top: 0;
        left: 0;
        width: 100%;
        height: 100%;
        overflow: hidden;
        /* The plot handles the pointer. A hit test on the long profile paths costs more than the rest of a hover step. */
        pointer-events: none;
    }
    .height {
        position: absolute;
        left: -48px;
        transform: translateY(-50%);
        font-size: 11px;
        color: var(--ink-faint);
        font-variant-numeric: tabular-nums;
    }
    .grid {
        fill: none;
        stroke: var(--line);
        stroke-width: 1;
        vector-effect: non-scaling-stroke;
    }
    .terrain-fill { fill: var(--terrain); }
    .day-fill { fill: color-mix(in srgb, var(--day-color) 60%, var(--ink-faint)); opacity: .10; }
    .day-fill.quiet { fill: var(--ink-faint); }
    path:not(.grid):not(.terrain-fill):not(.day-fill) {
        fill: none;
        stroke-width: 2;
        vector-effect: non-scaling-stroke;
    }
    path.grade-line { stroke-width: 2.5; stroke-linejoin: round; }
    line {
        stroke: var(--ink-faint);
        stroke-width: 1;
        stroke-dasharray: 3 4;
        vector-effect: non-scaling-stroke;
    }
    line.pinned {
        stroke: var(--ink);
        stroke-dasharray: none;
    }
    .handle {
        position: absolute;
        z-index: 2;
        bottom: 4px;
        width: 26px;
        height: 26px;
        display: grid;
        place-items: center;
        padding: 0;
        transform: translateX(-50%);
        border: 2px dashed var(--ink);
        border-radius: 50%;
        background: var(--panel);
        color: var(--ink);
        font: 700 12px var(--sans);
        cursor: ew-resize;
        touch-action: none;
    }
    .handle.moved {
        border-style: dotted;
    }
    .handle.moved::after {
        content: "";
        position: absolute;
        bottom: 2px;
        width: 3px;
        height: 3px;
        border-radius: 50%;
        background: currentColor;
    }
    .handle.pinned {
        border: 2px solid var(--panel);
        background: var(--day-color);
        color: var(--panel);
        cursor: pointer;
    }
    .handle.active {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
    }
    .handle:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 3px;
    }
</style>
