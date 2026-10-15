<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { openingHoursRows } from '../../lib/planner/search/opening-hours';
    import { clockTime, nightLow, sunsetAt } from '../../lib/planner/evening';
    import { stopCounts, type Stop, type StopPlace } from '../../lib/planner/stops';
    import type { Day, Place } from '../../lib/planner/editor';

    let { stop, days, labels, theme, date, elevation, onBack, onEndHere, onStay, onSelectPlace }: {
        stop: Stop;
        /** The day that ends here and the day that starts here. */
        days: [Day, Day];
        labels: [number, number];
        theme: 'light' | 'dark';
        date?: string;
        elevation: number | null;
        onBack: () => void;
        onEndHere: (stop: Stop) => void;
        onStay: (place: Place) => void;
        onSelectPlace: (place: Place) => void;
    } = $props();

    const groups = $derived([
        { title: 'Campsites', places: stop.sleep.filter(p => p.place.category === 'camp') },
        { title: 'Hotels', places: stop.sleep.filter(p => p.place.category === 'hotel') },
        { title: 'Shelters', places: stop.sleep.filter(p => p.place.category === 'shelter') },
    ].filter(group => group.places.length));
    const center = $derived(stop.sleep[0].place.coordinate);
    const sunset = $derived(date ? sunsetAt(center, date) : null);
    let low = $state('');
    $effect(() => {
        low = '';
        if (!date) return;
        const abort = new AbortController();
        nightLow(center, elevation, date, abort.signal).then(text => { if (!abort.signal.aborted) low = text; }, () => {});
        return () => abort.abort();
    });

    const kind = (p: StopPlace) => p.place.placeKind ? kindLabel(p.place.placeKind) : placeCategories[p.place.category].label.toLowerCase();
    // Search names an unnamed place after its kind.
    const named = (p: StopPlace) => p.place.label.toLowerCase() !== kind(p);
    const names = (places: StopPlace[]) => places.slice(0, 3).map(p => p.place.label).join(', ');
    const away = (p: StopPlace) => p.off < .15 ? 'on the route' : `${p.off.toFixed(1)} km off the route`;
    function shops(places: StopPlace[]) {
        return places.slice(0, 3).map(({ place }) => {
            const hours = place.openingHours ? openingHoursRows(place.openingHours)?.[0] : null;
            return hours ? `${place.label} (${hours.days} ${hours.periods.join(', ')})` : place.label;
        }).join(', ');
    }
    function duration(hours: number) {
        const minutes = Math.round(hours * 60);
        return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, '0')}m`;
    }
</script>

<div class="card">
    <button type="button" class="back" onclick={onBack} {@attach node => node.focus({ preventScroll: true })}><Icon name="back" size={15} />All towns</button>
    <header>
        <h3>{stop.name}</h3>
        <p>km {Math.round(stop.from)}{Math.round(stop.to) !== Math.round(stop.from) ? `–${Math.round(stop.to)}` : ''} · {stopCounts(stop)}</p>
    </header>
    <dl class="days">
        {#each days as day, i}
            <div style:--day={dayColor(day.number, theme)}>
                <dt><span class="badge">{labels[i]}</span>Day {labels[i]} {i ? 'starts here' : 'ends here'}</dt>
                <dd>{day.distance.toFixed(1)} km · {duration(day.hours)} · ↑ {Math.round(day.ascent)} m</dd>
            </div>
        {/each}
    </dl>
    <button type="button" class="end-here" onclick={() => onEndHere(stop)}>End day {labels[0]} in {stop.name}</button>

    {#each groups as group (group.title)}
        <h4>{group.title}</h4>
        <ul>
            {#each group.places as entry (entry.place.id)}
                <li>
                    <button type="button" class="place" onclick={() => onSelectPlace(entry.place)}>
                        <Icon path={placeCategories[entry.place.category].icon} size={16} />
                        <span><strong>{named(entry) ? entry.place.label : `Unnamed ${kind(entry)}`}</strong><small>{named(entry) ? `${kind(entry)} · ` : ''}{away(entry)}</small></span>
                    </button>
                    <button type="button" class="planner-action stay" onclick={() => onStay(entry.place)} aria-label={`Stay at ${entry.place.label}`}>Stay here</button>
                </li>
            {/each}
        </ul>
    {/each}

    <h4>Evening and morning</h4>
    <ul class="facts">
        <li><Icon path={placeCategories.food.icon} size={15} /><span><b>Food</b> {stop.food.length ? names(stop.food) : 'None mapped nearby'}</span></li>
        <li><Icon path={placeCategories.shop.icon} size={15} /><span><b>Shops</b> {stop.shop.length ? shops(stop.shop) : 'None mapped nearby'}</span></li>
        <li><Icon path={placeCategories.water.icon} size={15} /><span><b>Water</b> {stop.water.length ? `${stop.water[0].place.label}${stop.water.length > 1 ? ` and ${stop.water.length - 1} more` : ''}` : 'None mapped nearby'}</span></li>
        {#if date}
            <li><Icon name="moon" size={15} /><span><b>Night</b> {sunset ? `Sunset ${clockTime(sunset)}` : 'No sunset'}{low ? ` · ${low}` : ''}</span></li>
        {:else}
            <li><Icon name="moon" size={15} /><span>Set a start date to see the sunset and the usual night temperature.</span></li>
        {/if}
    </ul>
</div>

<style>
    .card {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    button:focus-visible {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
    }
    .back {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        align-self: flex-start;
        margin-left: -4px;
        padding: 4px;
        border-radius: 6px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .back:hover {
        color: var(--ink);
    }
    header {
        margin-top: 4px;
    }
    h3 {
        margin: 0;
        font: 600 17px/1.3 var(--sans);
        overflow-wrap: anywhere;
    }
    header p {
        margin: 2px 0 0;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .days {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 8px;
        margin: 4px 0 0;
    }
    .days div {
        padding: 8px 10px;
        border-radius: 8px;
        background: var(--parchment-2);
    }
    dt {
        display: flex;
        align-items: center;
        gap: 6px;
        font-size: 12px;
        color: var(--ink-soft);
    }
    dd {
        margin: 4px 0 0;
        font: 600 13px/1.35 var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .badge {
        width: 18px;
        height: 18px;
        flex: none;
        display: grid;
        place-items: center;
        border-radius: 50%;
        background: var(--day);
        color: var(--panel);
        font: 700 11px var(--sans);
    }
    .end-here {
        align-self: flex-start;
        min-height: 32px;
        padding: 0 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        font-size: 13px;
        font-weight: 600;
    }
    .end-here:hover {
        background: var(--parchment-2);
    }
    h4 {
        margin: 12px 0 0;
        font: 600 13px var(--sans);
        color: var(--ink-soft);
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: flex;
        align-items: center;
        gap: 8px;
    }
    li + li {
        border-top: 1px solid var(--line);
    }
    .place {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        gap: 10px;
        margin-left: -8px;
        padding: 8px;
        border-radius: 6px;
        text-align: left;
    }
    .place:hover {
        background: var(--parchment-2);
    }
    .place > :global(svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .place span {
        min-width: 0;
    }
    .place strong,
    .place small {
        display: block;
        overflow-wrap: anywhere;
    }
    .place strong {
        font: 600 14px/1.3 var(--sans);
    }
    .place small {
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .stay {
        flex: none;
        font-size: 13px;
    }
    .facts li {
        align-items: flex-start;
        padding: 6px 0;
        font-size: 13px;
        line-height: 1.45;
        color: var(--ink);
    }
    .facts li + li {
        border-top: 0;
    }
    .facts :global(svg) {
        flex: none;
        margin-top: 2px;
        color: var(--ink-soft);
    }
    .facts b {
        font-weight: 600;
        margin-right: 4px;
    }
</style>
