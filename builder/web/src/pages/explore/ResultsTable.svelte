<!-- The results of the day-hike search under the map: the count, a table that sorts by a click on a column header, a row
     that opens in place, and for an empty list the changes that help. -->
<script lang="ts">
    import { tick, untrack } from 'svelte';
    import { on } from 'svelte/events';
    import { routeColor } from '../../lib/planner/route-colors';
    import type { CatalogRecord, FactsPlace } from '../../lib/planner/signed-routes';
    import { formatDistance, formatHeight, formatHours } from '../../lib/settings/format';
    import Icon from '../../lib/ui/Icon.svelte';
    import type { SignedItem } from '../routes/filter';
    import MiniProfile from '../routes/MiniProfile.svelte';
    import { firstRows, ROWS } from './rows';
    import { wishesOf, type Query, type Row, type Sort, type Suggestion } from './search';
    import ShapeIcon from './ShapeIcon.svelte';
    import { kindName } from './SignedCard.svelte';
    import SignedPicture from './SignedPicture.svelte';
    import { accessSteps, accessText, hikes, placeName, SHAPE_WORDS, shapeText, snowText, suggestionText, wishIcon, wishLabel } from './words';

    let { rows, note, waiting, none, query, sort, opened, suggestions, place, itemOf, linkOf, oncover, onsort, onopen, onquery, onreset, onplan, onretry }: {
        /** In their order; undefined while there is no list, and `note` says why. */
        rows: Row[] | undefined;
        note: string;
        /** The first answer loads: grey rows stand for the hikes. */
        waiting: boolean;
        /** The map view holds no hike. */
        none: boolean;
        query: Query;
        sort: Sort;
        opened: Row | undefined;
        /** The changes that give an empty list rows. */
        suggestions: Suggestion[];
        place: (id: string) => FactsPlace | undefined;
        itemOf: (record: CatalogRecord) => SignedItem;
        /** The page of a route: the name of an open row links to it. */
        linkOf: (record: CatalogRecord) => string | undefined;
        oncover: (credits: string[] | undefined) => void;
        onsort: (sort: Sort) => void;
        onopen: (row: Row | undefined) => void;
        onquery: (query: Query) => void;
        onreset: () => void;
        onplan: (record: CatalogRecord) => void;
        onretry?: () => void;
    } = $props();

    const COLUMNS: { sort: Sort; label: string; class?: string }[] = [
        { sort: 'name', label: 'Hike' }, { sort: 'shape', label: 'Shape', class: 'shape' }, { sort: 'length', label: 'Length', class: 'num' },
        { sort: 'climb', label: 'Climb', class: 'num' }, { sort: 'time', label: 'Time', class: 'num' }, { sort: 'grade', label: 'Grade' },
        { sort: 'access', label: 'Getting there' }, { sort: 'match', label: 'Best fit', class: 'fit' },
    ];
    const nice = $derived(wishesOf(query, 'nice'));
    const key = (row: Row) => `${row.record.id}/${row.shape}`;
    /** The grade of a route; the table says "Unknown" for none, the open row "Grade unknown". */
    const grade = (record: CatalogRecord, none = 'Grade unknown') => record.hardest === undefined ? none : `T${record.hardest + 1}`;

    // The table draws its first rows, and more when the rider scrolls near their end.
    let scroller = $state<HTMLElement>(), body = $state<HTMLElement>(), limit = $state(ROWS);
    const openAt = $derived(opened && rows ? rows.indexOf(opened) : -1);
    const drawn = $derived(rows && rows.length > limit ? rows.slice(0, limit) : rows ?? []);
    function drawMore() {
        if (scroller && rows && limit < rows.length && scroller.scrollTop + scroller.clientHeight > scroller.scrollHeight - 600) limit += ROWS;
    }
    // A new list starts at its top, with the open row drawn.
    $effect.pre(() => {
        void rows;
        limit = firstRows(untrack(() => openAt));
        if (scroller) scroller.scrollTop = 0;
    });
    // A row opened from outside the drawn rows, as from the map, is drawn.
    $effect.pre(() => {
        if (openAt >= 0) limit = Math.max(untrack(() => limit), openAt + 1);
    });
    // Only the table scrolls, not the page, to show the open row whole.
    $effect(() => {
        void [rows, opened];
        void tick().then(() => {
            const row = body?.querySelector<HTMLElement>('tr.open');
            if (!row || !scroller) return;
            const head = scroller.querySelector('thead')!.offsetHeight, more = row.nextElementSibling as HTMLElement | null;
            const top = row.offsetTop - head, bottom = (more ?? row).offsetTop + (more ?? row).offsetHeight;
            if (top < scroller.scrollTop) scroller.scrollTop = top;
            else if (bottom > scroller.scrollTop + scroller.clientHeight) scroller.scrollTop = Math.min(top, bottom - scroller.clientHeight);
        });
    });
    // A click anywhere on a row opens it, or closes the open one; the name is its button for the keys.
    $effect(() => {
        if (!body) return;
        return on(body, 'click', event => {
            const target = event.target as Element, row = target.closest<HTMLElement>('tr.row');
            if (!row || target.closest('a')) return;
            const found = rows?.find(one => key(one) === row.dataset.key);
            onopen(found === opened ? undefined : found);
        });
    });

    function close() {
        const row = opened && key(opened);
        onopen(undefined);
        void tick().then(() => body?.querySelector<HTMLElement>(`tr[data-key="${row}"] .name`)?.focus());
    }

    /** The places that the route passes, by name: summits, huts, lakes and waterfalls. */
    function passes(row: Row): string {
        const names = (row.facts?.places ?? []).map(([id]) => placeName(place(id)));
        return names.length > 5 ? `${names.slice(0, 5).join(', ')} and ${names.length - 5} more` : names.join(', ');
    }
</script>

<div class="results">
    {#if rows}
        <div class="top">
            <h2 class="count" aria-live="polite">{hikes(rows.length)}</h2>
        </div>
    {:else}
        <p class="top note" aria-live="polite">{note}{#if onretry}{' '}<button type="button" class="retry" onclick={onretry}>Retry</button>{/if}</p>
    {/if}
    <div class="scroll" bind:this={scroller} onscroll={drawMore}>
        <table class:no-wishes={!nice.length}>
            <thead>
                <tr>
                    {#each COLUMNS as column (column.sort)}
                        <th scope="col" class={column.class} aria-sort={sort === column.sort ? column.sort === 'match' ? 'descending' : 'ascending' : undefined}>
                            <button type="button" onclick={() => onsort(column.sort)}>{column.label}<Icon name="down" size={14} /></button>
                        </th>
                    {/each}
                </tr>
            </thead>
            <tbody bind:this={body}>
                {#each drawn as row (key(row))}
                    {@const record = row.record}
                    {@const open = row === opened}
                    <tr class="row" class:open data-key={key(row)} data-line={record.id}>
                        <td>
                            <button type="button" class="name" aria-expanded={open} data-line={record.id}>
                                <span class="swatch" style:--colour={routeColor(String(record.id))}></span>
                                <span class="names"><strong>{record.name ?? record.ref ?? ''}</strong>{#if 'made' in record}<span class="ours">Ours</span>{/if}</span>
                            </button>
                        </td>
                        <td class="shape"><span title={SHAPE_WORDS[row.shape].tag}><ShapeIcon shape={row.shape} size={16} /><span class="word">{SHAPE_WORDS[row.shape].tag}</span></span></td>
                        <td class="num">{formatDistance(row.length_m)}</td>
                        <td class="num">{formatHeight(row.ascent_m)}</td>
                        <td class="num">{row.moving_s === undefined ? '–' : formatHours(row.moving_s)}</td>
                        <td>{grade(record, 'Unknown')}</td>
                        <td><span class="steps">{#each accessSteps(row, place) as step, i (i)}<span class="step" class:plain={!step.icon && !step.park}>{#if step.park}<span class="park" aria-hidden="true">P</span>{:else if step.icon}<Icon name={step.icon} size={14} />{/if}{step.text}</span>{/each}</span></td>
                        <td class="fit">
                            <span class="wishes" aria-hidden="true">
                                {#each nice as wish (wish)}
                                    {@const met = row.met.includes(wish)}
                                    <span class="wish" class:met title="{met ? 'Has' : row.unknown.includes(wish) ? 'No data' : 'Missing'}: {wishLabel(wish, place)}"><Icon name={wishIcon(wish, place)} size={15} /></span>
                                {/each}
                            </span>
                            <span class="hidden">{row.met.length} of {nice.length} wishes</span>
                        </td>
                    </tr>
                    {#if open}
                        <tr class="more"><td colspan={COLUMNS.length}>{@render more(row)}</td></tr>
                    {/if}
                {/each}
                {#if rows && !rows.length}
                    <tr><td colspan={COLUMNS.length}>{@render empty()}</td></tr>
                {:else if waiting}
                    {#each { length: 4 }}<tr class="waiting" aria-hidden="true"><td colspan={COLUMNS.length}><span></span></td></tr>{/each}
                {/if}
            </tbody>
        </table>
    </div>
</div>

{#snippet more(row: Row)}
    {@const record = row.record}
    {@const item = itemOf(record)}
    {@const link = linkOf(record)}
    {@const facts = row.facts}
    {@const snow = snowText(facts?.snow_free)}
    {@const lift = row.shape === 'liftDown' || row.shape === 'liftUp'}
    {@const how = accessText(row, place)}
    <div class="more-box">
        <div class="cover"><SignedPicture {item} {oncover} /></div>
        <div class="column">
            <h3>{#if link}<a href={link}>{item.name}</a>{:else}{item.name}{/if}</h3>
            <p class="numbers">{[formatDistance(row.length_m), `${formatHeight(row.ascent_m)} up`, ...lift ? [`${formatHeight(row.descent_m)} down`] : [],
                ...row.moving_s === undefined ? [] : [formatHours(row.moving_s)], grade(record)].join(' · ')}</p>
            <p class="kind">{shapeText(row)} · {'made' in record ? `Our ${record.loop ? 'loop' : 'route'}, made from signed hiking paths` : kindName(record)}</p>
            {#if record.credit}<p class="kind">Route data: {record.credit}</p>{/if}
            {#if how}<p>{how}</p>{/if}
            {#if row.met.length + row.missed.length + row.unknown.length}
                <p class="tags">
                    {#each row.met as wish (wish)}<span><Icon name="check" size={12} />{wishLabel(wish, place, true)}</span>{/each}
                    {#each row.missed as wish (wish)}<span class="no"><Icon name="close" size={11} />{wishLabel(wish, place, true)}</span>{/each}
                    {#each row.unknown as wish (wish)}<span class="no">? {wishLabel(wish, place, true)}</span>{/each}
                </p>
            {/if}
            <div class="profile"><MiniProfile summary={item.summary} /></div>
        </div>
        <div class="column">
            <ul class="facts">
                {#if item.summary.highestM !== null}<li><Icon name="mountain" size={16} />Highest point {formatHeight(item.summary.highestM)}</li>{/if}
                {#if snow}<li><Icon name="snow" size={16} />{snow}</li>{/if}
                {#if facts}
                    <li><Icon name="paved" size={16} />{facts.paved_pct} % paved</li>
                    {#if facts.canopy_pct !== undefined}<li><Icon name="tree" size={16} />{facts.canopy_pct} % forest</li>{/if}
                    {#if facts.big_road_pct > 5}<li><Icon name="quiet" size={16} />{facts.big_road_pct} % near big roads</li>{/if}
                    {#if facts.places?.length}<li><Icon name="pin" size={16} />Passes {passes(row)}</li>{/if}
                {/if}
            </ul>
            <div class="actions">
                <button type="button" class="button primary" onclick={() => onplan(record)}>Plan this route</button>
                <button type="button" class="text" onclick={close}>Close</button>
            </div>
        </div>
    </div>
{/snippet}

{#snippet empty()}
    <div class="empty">
        {#if none}
            <strong>No hike in this map view.</strong>
        {:else}
            <strong>No hike fits all of this.</strong>
            {#if suggestions.length}
                <span>One change would help:</span>
                <div>
                    {#each suggestions as suggestion, i (i)}
                        <button type="button" class="button" onclick={() => onquery(suggestion.query)}>{suggestionText(suggestion, query, place)}<span class="n">{hikes(suggestion.count)}</span></button>
                    {/each}
                </div>
            {:else}
                <span>Widen the length or the climb.</span>
            {/if}
            <button type="button" class="text" onclick={onreset}>Reset the filters</button>
        {/if}
    </div>
{/snippet}

<style>
    .results {
        display: flex;
        flex: 1;
        flex-direction: column;
        min-height: 0;
        font-variant-numeric: tabular-nums;
    }
    .top {
        display: flex;
        flex: none;
        align-items: baseline;
        gap: 12px;
        margin: 0;
        padding: 4px 16px 10px;
    }
    .count {
        margin: 0;
        font-size: 15px;
        font-weight: 700;
        white-space: nowrap;
    }
    .note {
        color: var(--ink-soft);
        font-size: 13px;
    }
    .retry {
        padding: 0 4px;
        border: 0;
        background: none;
        color: var(--ink);
        font: inherit;
        text-decoration: underline;
    }
    .scroll {
        flex: 1;
        min-height: 0;
        overflow: auto;
    }
    table {
        width: 100%;
        border-collapse: separate;
        border-spacing: 0;
    }
    th, td {
        padding: 0 10px;
        white-space: nowrap;
    }
    th:first-child, td:first-child {
        padding-left: 16px;
    }
    th:last-child, td:last-child {
        padding-right: 16px;
    }
    thead th {
        position: sticky;
        z-index: 1;
        top: 0;
        height: 34px;
        border-block: 1px solid var(--line);
        background: var(--panel);
        color: var(--ink-soft);
        font-size: 12.5px;
        font-weight: 600;
        text-align: left;
    }
    .num {
        text-align: right;
    }
    th button {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        margin: 0 -6px;
        padding: 3px 6px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: inherit;
        font: inherit;
    }
    th.num button {
        flex-direction: row-reverse;
    }
    th button :global(svg) {
        opacity: 0;
    }
    th button:hover {
        background: var(--parchment);
        color: var(--ink);
    }
    th button:hover :global(svg) {
        opacity: .45;
    }
    th[aria-sort] button {
        color: var(--ink);
    }
    th[aria-sort] button :global(svg) {
        opacity: 1;
    }
    .row td {
        height: 46px;
        border-bottom: 1px solid var(--line);
        cursor: pointer;
    }
    /* The frame marks the rows of the line under the pointer. */
    .row:hover td, .row:global([data-hover]) td {
        background: var(--parchment);
    }
    .row.open td {
        border-bottom-color: transparent;
        background: var(--parchment-2);
    }
    .name {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        min-height: 44px;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
    }
    .name:focus-visible {
        outline-offset: -2px;
    }
    .swatch {
        flex: none;
        width: 16px;
        height: 4px;
        border-radius: 2px;
        background: var(--colour);
    }
    .names {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 2px 8px;
        min-width: 0;
        white-space: normal;
    }
    .names strong {
        font-weight: 600;
    }
    .ours {
        padding: 0 6px;
        border: 1px solid var(--line-strong);
        border-radius: 4px;
        color: var(--ink-soft);
        font-size: 11.5px;
        font-weight: 500;
    }
    .shape span {
        display: inline-flex;
        align-items: center;
        gap: 7px;
        color: var(--ink-soft);
        font-size: 13px;
    }
    /* The steps wrap before the table grows wider than its panel. */
    .steps {
        display: inline-flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 0 4px;
        font-size: 13px;
    }
    .step {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        white-space: nowrap;
    }
    .step + .step::before {
        content: "›";
        margin-right: 4px;
        color: var(--ink-soft);
    }
    .step :global(svg), .plain {
        color: var(--ink-soft);
    }
    .park {
        display: inline-grid;
        flex: none;
        place-items: center;
        width: 16px;
        height: 16px;
        border-radius: 4px;
        background: var(--water);
        color: var(--panel);
        font: 700 10.5px/1 var(--sans);
    }
    .wishes {
        display: flex;
        gap: 4px;
    }
    .wish {
        display: inline-grid;
        place-items: center;
        width: 26px;
        height: 26px;
        border-radius: 6px;
        color: var(--ink-soft);
        opacity: .3;
    }
    .wish.met {
        background: var(--parchment-2);
        color: var(--ink);
        opacity: 1;
    }
    .row.open .wish.met, .row:hover .wish.met {
        background: var(--panel);
    }
    .no-wishes .fit {
        display: none;
    }
    .hidden {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
        white-space: nowrap;
    }
    .more td {
        padding: 0 16px 16px;
        border-bottom: 1px solid var(--line);
        background: var(--parchment-2);
        white-space: normal;
    }
    .more-box {
        display: grid;
        grid-template-columns: 200px minmax(0, 1.25fr) minmax(0, 1fr);
        gap: 20px;
        padding: 14px;
        border-radius: 10px;
        background: var(--panel);
    }
    .cover {
        height: 136px;
        overflow: hidden;
        border-radius: 8px;
    }
    .column {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 8px;
        min-width: 0;
    }
    .column p {
        margin: 0;
        font-size: 13.5px;
    }
    h3 {
        margin: 0;
        font-size: 15px;
    }
    h3 a {
        color: var(--ink);
    }
    .numbers {
        font-weight: 600;
    }
    .kind {
        color: var(--ink-soft);
    }
    .tags {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 12px;
    }
    .tags span {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        white-space: nowrap;
    }
    .tags .no {
        color: var(--ink-soft);
    }
    .profile {
        width: 100%;
        height: 44px;
        margin-top: auto;
    }
    .facts {
        display: grid;
        gap: 6px;
        margin: 0;
        padding: 0;
        font-size: 13px;
        list-style: none;
    }
    .facts li {
        display: flex;
        align-items: center;
        gap: 8px;
    }
    .facts :global(svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .actions {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px;
        margin-top: auto;
    }
    .button {
        display: inline-flex;
        align-items: center;
        gap: 7px;
        min-height: 32px;
        padding: 0 12px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
        font: 13px var(--sans);
        transition: transform 160ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .button:hover {
        border-color: var(--ink-soft);
    }
    .button:active {
        transform: scale(0.97);
    }
    .primary {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
        font-weight: 600;
    }
    .text {
        padding: 4px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: 600 13px var(--sans);
    }
    .text:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .empty {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 10px;
        padding: 20px 0;
        color: var(--ink-soft);
        white-space: normal;
    }
    .empty strong {
        color: var(--ink);
        font-size: 15px;
    }
    .empty div {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }
    .n {
        margin-left: 4px;
        color: var(--ink-soft);
    }
    .waiting span {
        display: block;
        height: 14px;
        margin: 16px 0;
        border-radius: 4px;
        background: var(--parchment-2);
    }
    /* The shape column shows its picture alone. */
    @media (max-width: 1180px) {
        .word {
            position: absolute;
            width: 1px;
            height: 1px;
            overflow: hidden;
            clip-path: inset(50%);
        }
    }
    @media (max-width: 760px) {
        .more-box {
            grid-template-columns: minmax(0, 1fr);
        }
    }
</style>
