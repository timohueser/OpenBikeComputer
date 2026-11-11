<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { kindLabel, stayOptions } from '../../lib/planner/search/presentation';
    import { openingHoursRows } from '../../lib/planner/search/opening-hours';
    import { clockTime, nightLow, sunsetAt } from '../../lib/planner/evening';
    import { stopCounts, stopKm, suitable, type Stop, type StopPlace } from '../../lib/planner/stops';
    import type { Day, Place, Stay } from '../../lib/planner/editor';

    let { stop, kinds, days, labels, back = 'All towns', shownId = null, theme, date, elevation, onBack, onEndHere, onStay, onSelectPlace }: {
        stop: Stop;
        /** The kinds of place to sleep of the night: the card lists only places of these kinds. */
        kinds: Stay[];
        /** The day that ends here and the day that starts here. */
        days: [Day, Day];
        labels: [number, number];
        back?: string;
        /** The id of the place or point that the map shows. */
        shownId?: string | null;
        theme: 'light' | 'dark';
        date?: string;
        elevation: number | null;
        onBack: () => void;
        onEndHere: (stop: Stop) => void;
        onStay: (place: Place) => void;
        onSelectPlace: (place: Place | null) => void;
    } = $props();

    const groups = $derived(stayOptions.map(({ kind, label }) => ({ title: label, places: kinds.includes(kind) ? suitable(stop, [kind]) : [] }))
        .filter(group => group.places.length));
    const radios = $props.id();
    // The id of the selected place; null selects the town itself.
    let picked = $state<string | null>(null);
    const places = $derived(groups.flatMap(group => group.places));
    const selected = $derived(places.find(p => p.place.id === picked));
    // A place of another town, or one that the stay kinds hide, gives the selection back to the town.
    $effect(() => { if (picked !== null && !selected) picked = null; });
    // A place of the card picked on the map selects it here too; a closed callout keeps the selection.
    $effect(() => { if (shownId && places.some(p => p.place.id === shownId)) picked = shownId; });
    function select(entry: StopPlace | null) {
        picked = entry?.place.id ?? null;
        onSelectPlace(entry?.place ?? null);
    }
    // A checked radio fires no change, so a click on it shows its place again.
    function again(entry: StopPlace | null) {
        if ((entry ?? undefined) === selected) select(entry);
    }
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

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="card" onkeydown={event => { if (event.key === 'Escape') { event.stopPropagation(); onBack(); } }}>
    <button type="button" class="back" onclick={onBack} {@attach node => node.focus({ preventScroll: true })}><Icon name="back" size={15} />{back}</button>
    <header>
        <h3>{stop.name}</h3>
        <p>{stopKm(stop, kinds)} · {stopCounts(stop, kinds)}</p>
    </header>
    <dl class="days">
        {#each days as day, i}
            <div style:--day={dayColor(day.number, theme)}>
                <dt><span class="badge">{labels[i]}</span>Day {labels[i]} {i ? 'starts here' : 'ends here'}</dt>
                <dd>{day.distance.toFixed(1)} km · {duration(day.hours)} · ↑ {Math.round(day.ascent)} m</dd>
            </div>
        {/each}
    </dl>

    <h4>Where you sleep</h4>
    <div role="radiogroup" aria-label={`Where you sleep on night ${labels[0]}`}>
        <label class="option" class:checked={!selected}>
            <input type="radio" name={radios} checked={!selected} onchange={() => select(null)} onclick={() => again(null)} />
            <span><strong>Somewhere in {stop.name}</strong><small>decide the place later</small></span>
        </label>
        {#each groups as group (group.title)}
            <h4>{group.title}</h4>
            <ul>
                {#each group.places as entry (entry.place.id)}
                    <li>
                        <label class="option" class:checked={selected === entry}>
                            <input type="radio" name={radios} checked={selected === entry} onchange={() => select(entry)} onclick={() => again(entry)} />
                            <span><strong>{named(entry) ? entry.place.label : `Unnamed ${kind(entry)}`}</strong><small>{named(entry) ? `${kind(entry)} · ` : ''}{away(entry)}</small></span>
                            <Icon path={placeCategories[entry.place.category].icon} size={16} />
                        </label>
                    </li>
                {/each}
            </ul>
        {/each}
    </div>

    <h4>Evening and morning</h4>
    <ul class="facts">
        <li><Icon path={placeCategories.food.icon} size={15} /><span><b>Food</b> {stop.food.length ? names(stop.food) : 'None mapped nearby'}</span></li>
        <li><Icon path={placeCategories.shop.icon} size={15} /><span><b>Shops</b> {stop.shop.length ? shops(stop.shop) : 'None mapped nearby'}</span></li>
        {#if date}
            <li><Icon name="moon" size={15} /><span><b>Night</b> {sunset ? `Sunset ${clockTime(sunset)}` : 'No sunset'}{low ? ` · ${low}` : ''}</span></li>
        {:else}
            <li><Icon name="moon" size={15} /><span>Set a start date to see the sunset and the usual night temperature.</span></li>
        {/if}
    </ul>
    <!-- Sticky inside the scrolling side panel, so the one way to confirm stays in view; it ends the card, so it never
         covers the last row. -->
    <div class="confirm">
        <button type="button" onclick={() => selected ? onStay(selected.place) : onEndHere(stop)}>
            End day {labels[0]} {selected ? `at ${named(selected) ? selected.place.label : `this ${kind(selected)}`}` : `in ${stop.name}`}
        </button>
    </div>
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
    .option {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        gap: 10px;
        margin-left: -8px;
        padding: 8px;
        border-radius: 6px;
        cursor: pointer;
    }
    .option:hover,
    .option.checked {
        background: var(--parchment-2);
    }
    .option:has(:focus-visible) {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
    }
    .option input {
        flex: none;
        width: 16px;
        height: 16px;
        margin: 0;
        accent-color: var(--ink);
    }
    .option > span {
        flex: 1;
        min-width: 0;
    }
    .option > :global(svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .option strong,
    .option small {
        display: block;
        overflow-wrap: anywhere;
    }
    .option strong {
        font: 600 14px/1.3 var(--sans);
    }
    .option small {
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .confirm {
        position: sticky;
        bottom: 0;
        margin-top: 4px;
        padding: 12px 0;
        background: var(--panel);
    }
    .confirm button {
        width: 100%;
        min-height: 40px;
        padding: 8px 12px;
        border-radius: 6px;
        background: var(--ink);
        color: var(--panel);
        font-weight: 600;
        overflow-wrap: anywhere;
    }
    .confirm button:hover {
        filter: brightness(1.15);
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
