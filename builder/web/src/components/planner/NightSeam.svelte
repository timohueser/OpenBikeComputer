<!-- The night between two day cards: the stops where the day can end, in route order on the line of the route. A stop opens
     in place to its places to sleep, and one button ends the day there. -->
<script lang="ts">
    import { untrack } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import StayChip from './StayChip.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { allStay, duration, type Day, type Trip } from '../../lib/planner/editor';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { kindLabel, sleepIcon, stayIcon, stayKind } from '../../lib/planner/search/presentation';
    import { daysAround, evenStop, proposal, stayNouns, stopCounts, suitable, type Stop, type StopPlace } from '../../lib/planner/stops';
    import { distanceIn } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import { still, unfold } from '../../lib/ui/search-bar/motion';

    let { trip, line, day, theme, nights, onEnd }: {
        trip: Trip;
        line?: RoutingLine;
        /** The day that ends at this night, as the trip has it. */
        day: Day;
        theme: 'light' | 'dark';
        nights: NightChoices;
        /** Tells where the day ends in view, in km, and the town there; null is the day end of the trip. */
        onEnd: (end: { km: number; town: string } | null) => void;
    } = $props();

    interface Row {
        stop: Stop;
        /** The places of the kinds of the night, in route order. */
        places: StopPlace[];
        /** The one place of a stop that is no town. */
        lone?: StopPlace;
        /** Where the day ends at the stop, in km, and the two days around that end. */
        km: number;
        days: [Day, Day];
    }

    const night = $derived(day.number);
    const kinds = $derived(nights.kinds(night));
    const found = $derived(nights.found(night));
    const window = $derived(nights.window(night));
    const rows = $derived((found?.state === 'ready' ? found.stops : []).flatMap((stop): Row[] => {
        const places = suitable(stop, kinds).sort((a, b) => a.km - b.km);
        if (!places.length) return [];
        const lone = places.length === 1 ? places[0] : undefined, km = lone?.km ?? proposal(stop, kinds).km;
        return [{ stop, places, lone, km, days: daysAround(trip, line, night, km) }];
    }));
    const even = $derived(evenStop(rows.map(row => row.stop), trip, line, night));
    // Huts, hostels and hotels share one icon, so one count stands for them.
    const icons = [...new Set(allStay.map(stayIcon))];
    const counts = (places: StopPlace[]) => icons.map(icon => ({ icon, count: places.filter(p => sleepIcon(p.place.placeKind) === icon).length }))
        .filter(({ count }) => count);

    const kindOf = (p: StopPlace) => p.place.placeKind ? kindLabel(p.place.placeKind) : placeCategories[p.place.category].label.toLowerCase();
    // Search names an unnamed place after its kind.
    const named = (p: StopPlace) => p.place.label.toLowerCase() !== kindOf(p);
    const nameOf = (p: StopPlace) => named(p) ? p.place.label : `Unnamed ${kindOf(p)}`;
    const about = (p: StopPlace, where: string | false) => [named(p) && kindOf(p), where].filter(Boolean).join(' · ');
    const near = ({ stop, lone }: Row) => about(lone!, stop.name !== lone!.place.label && `near ${stop.name}`);
    const away = (p: StopPlace) => about(p, p.off < .15 ? 'on the route' : `${distanceIn(p.off * 1000).toFixed(1)} ${settings.units} off the route`);
    const isShelter = (p: StopPlace) => stayKind(p.place.placeKind) === 'shelter';
    const length = (km: number) => distanceIn(km * 1000).toFixed(1);

    const id = $props.id();
    // The stop under the pointer or the focus, and the selected place of the open stop; no place selects its town.
    let lit = $state<string | null>(null);
    let picked = $state<string | null>(null);
    let sheltersOpen = $state(false);
    const open = $derived(rows.find(row => row.stop.id === nights.stopId));
    const selected = $derived(open && (open.lone ?? open.places.find(p => p.place.id === picked)));
    // A place of the open stop picked on the map selects it here too; a closed callout keeps the selection.
    $effect(() => {
        const shown = nights.shownId;
        if (shown && open?.places.some(p => p.place.id === shown)) picked = shown;
    });
    const shelters = $derived(open?.places.filter(isShelter) ?? []);
    // Shelters fold into one row beside other places.
    const folded = $derived(shelters.length > 1 && shelters.length < (open?.places.length ?? 0));

    // The stop of the day end of the trip: the suggestion, or the stop that holds the place or the town that the rider set.
    const current = $derived.by(() => {
        const pinned = day.pinned;
        if (!pinned) return rows.find(row => row.stop.id === nights.around(night)?.stop.id);
        return rows.find(({ stop }) => stop.name === pinned.label || stop.sleep.some(p => p.place.id === pinned.placeId));
    });
    const viewed = $derived(open ?? rows.find(row => row.stop.id === lit));
    // The row of the day end in view carries the mark.
    const at = $derived(viewed ?? current);
    $effect(() => {
        onEnd(viewed ? { km: selected?.km ?? viewed.km, town: viewed.stop.name } : null);
        return () => onEnd(null);
    });

    // The stop whose block closed, so its row takes the focus back.
    let returnTo: string | null = null;
    function show(row: Row) {
        lit = picked = null;
        sheltersOpen = false;
        nights.open(row.stop.id);
    }
    function close(row: Row) {
        returnTo = row.stop.id;
        nights.open(null);
    }
    function select(place: StopPlace | null) {
        picked = place?.place.id ?? null;
        nights.selectPlace(place?.place ?? null);
    }
    // A checked radio fires no change, so a click on it shows its place again.
    function again(place: StopPlace | null) {
        if ((place ?? undefined) === selected) select(place);
    }

    let root = $state<HTMLElement>();
    /** Where the mark stands and the rail changes colour, in px from the top: at the node of the row in view; with no such
     * row, between the stops before the day end and the stops after it. Offsets, so a row that glides counts at its place. */
    function level(): number {
        const node = root!.querySelector<HTMLElement>('.at > .node'), list = root!.querySelector('ul');
        if (!node) {
            const next = list?.children[rows.filter(row => row.days[0].distance <= day.distance).length] as HTMLElement | undefined;
            return next ? next.offsetTop : list ? list.offsetTop + list.offsetHeight : root!.offsetHeight / 2;
        }
        let top = node.offsetHeight / 2;
        for (let part: Element | null = node; part instanceof HTMLElement && part !== root; part = part.offsetParent) top += part.offsetTop;
        return top;
    }
    // The mark glides to another row for 160 ms, and stays on its row when the rows around it move.
    let y = $state(0), glides = $state(false);
    let marked: string | null | undefined, glideEnd = 0;
    function place(row: string | null) {
        if (marked !== undefined && row !== marked) glideEnd = performance.now() + 160;
        marked = row;
        glides = performance.now() < glideEnd;
        y = level();
    }
    const markId = $derived(at ? `${at.stop.id} ${selected?.place.id ?? ''}` : null);
    $effect(() => {
        void [rows, sheltersOpen];
        const row = markId;
        untrack(() => place(row));
    });
    $effect(() => {
        const observer = new ResizeObserver(() => place(marked ?? null));
        observer.observe(root!);
        return () => observer.disconnect();
    });

    /** A new row fades in after the rows above it. Its parts fade, not the row, so its node stays over the rail. */
    function appear(row: HTMLElement) {
        if (still()) return;
        const delay = Math.min([...row.parentElement!.children].indexOf(row), 8) * 20;
        for (const part of (row.querySelector('.row') ?? row).children) {
            if (part.tagName !== 'INPUT') part.animate({ opacity: [0, 1] }, { duration: 180, delay, easing: 'cubic-bezier(.23, 1, .32, 1)', fill: 'backwards' });
        }
    }
</script>

{#snippet option(p: StopPlace)}
    <label class="row place" class:at={p === selected} {@attach appear}>
        <input type="radio" name={id} checked={p === selected} onchange={() => select(p)} onclick={() => again(p)} />
        <span class="node"></span>
        <Icon name={sleepIcon(p.place.placeKind)} size={16} />
        <span class="text"><strong>{nameOf(p)}</strong><small class="about">{away(p)}</small></span>
        <span class="fig"><b>{length(daysAround(trip, line, night, p.km)[0].distance)}<small>{settings.units}</small></b></span>
    </label>
{/snippet}

<!-- `data-glide` marks what the day list moves when a stop opens or closes. -->
<section class="night-seam" class:glides bind:this={root} aria-labelledby="{id}-title" style:--day={dayColor(night, theme)} style:--next={dayColor(night + 1, theme)}>
    <span class="rail" aria-hidden="true"><i style:transform={`translateY(${y}px)`}></i></span>
    {#if at}<span class="mark" aria-hidden="true" style:transform={`translateY(${y}px)`}>{night}</span>{/if}
    <header>
        <h2 id="{id}-title">Where day {night} ends</h2>
        <StayChip value={kinds} label={`Places to sleep on night ${night}`} onChange={kinds => nights.setKinds(night, kinds)}
            onReset={trip.nightStay?.[night] ? () => nights.setKinds(night, null) : undefined} resetLabel="Same as the trip" />
    </header>
    {#if !window}
        <p class="state" data-glide>The towns to sleep in appear when the route is ready.</p>
    {:else if found?.state === 'ready'}
        {#if rows.length}
            <ul aria-label={`Stops for night ${night}`}>
                {#each rows as row (row.stop.id)}
                    {@const { stop, lone, days } = row}
                    {@const title = lone ? nameOf(lone) : stop.name}
                    <li class:open={row === open} data-glide {@attach appear}>
                        {#if row === open}
                            <!-- svelte-ignore a11y_interactive_supports_focus -->
                            <div role="radiogroup" aria-label={`Where you sleep on night ${night}`}
                                onkeydown={event => { if (event.key === 'Escape') { event.stopPropagation(); close(row); } }}>
                                <div class="head">
                                    <label class="row" class:lone class:at={!!lone || !selected}>
                                        <input type="radio" name={id} checked={!!lone || !selected} onchange={() => select(null)} onclick={() => again(lone ?? null)}
                                            {@attach node => node.focus({ preventScroll: true })} />
                                        <span class="node"></span>
                                        <span class="text"><strong>{title}</strong><small class="about">{lone ? near(row) : `Anywhere in ${stop.name}, or a place:`}</small></span>
                                    </label>
                                    <button type="button" class="close" aria-label={`Close ${title}`} onclick={() => close(row)}><Icon name="up" size={16} /></button>
                                </div>
                                {#if !lone}
                                    {#each folded ? row.places.filter(p => !isShelter(p)) : row.places as p (p.place.id)}{@render option(p)}{/each}
                                    {#if folded}
                                        {@const shown = sheltersOpen || (!!selected && isShelter(selected))}
                                        <button type="button" class="row place fold" aria-expanded={shown} onclick={() => sheltersOpen = !shown}>
                                            <span></span><Icon name={stayIcon('shelter')} size={16} /><span class="text">{shelters.length} shelters</span><Icon name={shown ? 'up' : 'chevron'} size={14} />
                                        </button>
                                        {#if shown}<div transition:unfold>{#each shelters as p (p.place.id)}{@render option(p)}{/each}</div>{/if}
                                    {/if}
                                {/if}
                                <div class="foot">
                                    <button type="button" class="confirm" onclick={() => selected ? nights.stay(night, selected.place) : nights.endHere(night, stop)}>
                                        Sleep {selected ? `at ${named(selected) ? selected.place.label : `this ${kindOf(selected)}`}` : `in ${stop.name}`}
                                    </button>
                                </div>
                            </div>
                        {:else}
                            <button type="button" class="row" class:lone class:at={row === at} onclick={() => show(row)}
                                onmouseenter={() => lit = stop.id} onmouseleave={() => lit = null} onfocus={() => lit = stop.id} onblur={() => lit = null}
                                {@attach node => { if (stop.id === returnTo) { returnTo = null; node.focus(); } }}
                                aria-label={`${title}, ${lone ? near(row) : stopCounts(stop, kinds)}. Day ${night} ${duration(days[0].hours)}, day ${night + 1} ${duration(days[1].hours)}${stop === even ? '. Even days' : ''}`}>
                                <span class="node"></span>
                                <span class="text">
                                    <strong>{title}{#if stop === even}<em>Even days</em>{/if}</strong>
                                    {#if lone}
                                        <small class="about">{near(row)}</small>
                                    {:else}
                                        <small class="counts">{#each counts(row.places) as { icon, count } (icon)}<span><Icon name={icon} size={13} />{count}</span>{/each}</small>
                                    {/if}
                                </span>
                                <span class="fig"><b>{length(days[0].distance)}<small>{settings.units}</small></b><small>{duration(days[0].hours)}</small></span>
                            </button>
                        {/if}
                    </li>
                {/each}
            </ul>
        {:else}
            <p class="state" role="status" data-glide>No {kinds.length === 1 ? stayNouns[kinds[0]] : 'place to sleep'} between {settings.units} {Math.round(distanceIn(window[0] * 1000))} and {Math.round(distanceIn(window[1] * 1000))}.</p>
        {/if}
    {:else if found?.state === 'error'}
        <p class="state" role="status" data-glide>{found.message}</p>
    {:else}
        <p class="state" role="status" data-glide>Finding towns with somewhere to sleep…</p>
    {/if}
    <p class="actions" data-glide>
        {#if window && found?.state === 'error'}<button type="button" class="planner-action" onclick={() => nights.retry(night)}>Retry</button>{/if}
        <button type="button" class="planner-action quiet" onclick={() => nights.pick()}>Pick on the map</button>
        {#if day.pinned || day.split}<button type="button" class="planner-action quiet" onclick={() => nights.release(night)}>Let the planner choose</button>{/if}
    </p>
</section>

<style>
    /* The seam reaches its two cards across the gaps of the day list. The rail runs through the centre of the day numbers
       of the cards, over the fills of the rows and under their nodes. */
    .night-seam {
        position: relative;
        isolation: isolate;
        margin: -8px 0;
        color: var(--ink);
    }
    .rail {
        position: absolute;
        z-index: 1;
        top: 0;
        bottom: 0;
        left: 22px;
        width: 4px;
        overflow: hidden;
        background: var(--next);
    }
    .rail i {
        position: absolute;
        inset: auto 0 100%;
        height: 100%;
        background: var(--day);
    }
    .mark {
        position: absolute;
        z-index: 5;
        top: -13px;
        left: 11px;
        box-sizing: border-box;
        width: 26px;
        height: 26px;
        display: grid;
        place-items: center;
        border: 2px dashed var(--day);
        border-radius: 50%;
        background: var(--panel);
        color: var(--day);
        font: 700 12px var(--sans);
        pointer-events: none;
    }
    @media (prefers-reduced-motion: no-preference) {
        .glides :is(.rail i, .mark) { transition: transform 160ms cubic-bezier(.23, 1, .32, 1); }
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        justify-content: space-between;
        gap: 6px 8px;
        min-height: 44px;
        padding: 12px 4px 4px 50px;
    }
    h2 { margin: 0; font: 600 14px var(--sans); }
    ul { margin: 0; padding: 0; list-style: none; }
    /* A part that glides covers what it passes: the day list lifts it over the others. So each part is a sheet with its
       own piece of the rail. A sheet reaches one pixel under the next one, so no line shows between two that glide. */
    li, .state, .actions {
        position: relative;
        background: var(--panel);
        box-shadow: 0 1px var(--panel);
    }
    :is(li, .state, .actions)::after {
        content: "";
        position: absolute;
        top: 0;
        bottom: -1px;
        left: 22px;
        width: 4px;
        background: var(--next);
    }
    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    button:focus-visible, .row:has(:focus-visible) {
        outline: 2px solid var(--ink);
        outline-offset: -2px;
    }
    .row {
        position: relative;
        box-sizing: border-box;
        display: grid;
        grid-template-columns: 28px minmax(0, 1fr) auto;
        column-gap: 12px;
        align-items: center;
        width: 100%;
        min-height: 54px;
        padding: 5px 10px;
        border-radius: 10px;
        text-align: left;
        cursor: pointer;
    }
    .row.lone { min-height: 42px; }
    button.row:hover, button.row.at { background: var(--parchment-2); }
    .row input {
        position: absolute;
        opacity: 0;
        pointer-events: none;
    }
    .node {
        position: relative;
        z-index: 2;
        justify-self: center;
        box-sizing: border-box;
        width: 16px;
        height: 16px;
        border: 3px solid var(--ink);
        border-radius: 50%;
        background: var(--panel);
    }
    .lone .node, .place .node {
        width: 10px;
        height: 10px;
        border: 2px solid var(--ink-soft);
    }
    strong {
        display: block;
        font: 600 15px/1.3 var(--sans);
        overflow-wrap: anywhere;
    }
    .lone strong, .place strong { font-weight: 500; font-size: 14px; }
    small { font-size: 13px; color: var(--ink-soft); }
    .about { display: block; overflow-wrap: anywhere; }
    .about::first-letter { text-transform: uppercase; }
    .counts {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 10px;
        font-variant-numeric: tabular-nums;
    }
    .counts span {
        display: inline-flex;
        align-items: center;
        gap: 4px;
    }
    em {
        margin-left: 8px;
        padding: 1px 7px;
        border-radius: 999px;
        background: color-mix(in srgb, var(--amber) 24%, transparent);
        color: var(--forest-deep);
        font: 600 12px var(--sans);
        white-space: nowrap;
    }
    .fig {
        text-align: right;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .fig b { display: block; font: 700 15px var(--sans); }
    .fig b small { margin-left: 2px; font-weight: 400; }
    .lone .fig b, .place .fig b { font-weight: 600; font-size: 14px; }

    .open {
        margin: 2px 0;
        border-radius: 12px;
        background: var(--parchment-2);
    }
    .head {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        align-items: center;
    }
    .head .row { min-height: 50px; }
    .close {
        width: 36px;
        height: 36px;
        margin-right: 6px;
        display: grid;
        place-items: center;
        border-radius: 8px;
        color: var(--ink-soft);
    }
    .close:hover { color: var(--ink); }
    .place {
        grid-template-columns: 28px 18px minmax(0, 1fr) auto;
        column-gap: 10px;
        min-height: 46px;
    }
    .place > :global(svg) { color: var(--ink-soft); }
    /* The selected row keeps its node on the rail inside its ring. */
    label.at {
        width: calc(100% - 8px);
        margin: 0 4px;
        padding-left: 6px;
        padding-right: 6px;
        background: var(--panel);
        box-shadow: inset 0 0 0 1.5px var(--ink);
    }
    label.at strong { font-weight: 600; }
    .fold { color: var(--ink-soft); font: 500 14px var(--sans); }
    .fold > :global(svg:last-child) { color: var(--ink-faint); }
    /* Sticky inside the scrolling side panel, so the one way to confirm stays in view. It stands right of the rail, inside
       the rim of the block, and over the rows that scroll under it. */
    .foot {
        position: sticky;
        bottom: 0;
        z-index: 3;
        margin: 0 1px 1px 50px;
        padding: 8px 9px 9px 0;
        border-radius: 0 0 11px;
        background: var(--parchment-2);
    }
    .confirm {
        width: 100%;
        min-height: 40px;
        padding: 8px 12px;
        border-radius: 8px;
        background: var(--amber);
        color: var(--on-amber);
        font: 600 14.5px var(--sans);
        overflow-wrap: anywhere;
    }
    .confirm:hover { background: color-mix(in srgb, var(--amber) 86%, var(--ink)); }
    .state {
        margin: 0;
        padding: 4px 10px 8px 50px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .actions {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
        margin: 0;
        padding: 8px 10px 20px 50px;
        font-size: 13px;
    }
</style>
