<!-- The frame of a page with a list beside a map: the header with the title, the list in a card, the map, and the card
     of the selected item over the map, or a table under the map. On a phone the map is under the list, and the card is at
     the bottom of the screen. A row and the lines of its row light together: under the pointer on either side, or when a
     key focuses the row. -->
<script lang="ts">
    import { tick, type Snippet } from 'svelte';
    import { on } from 'svelte/events';
    import PanelResize from '../../components/planner/PanelResize.svelte';
    import Card from '../../lib/ui/Card.svelte';
    import type { Box, Item } from './filter';
    import { HoverState } from './hover.svelte';

    let { title, failure, items, shown, selected, marked, fit, place, look, credits, together, onselect, onview, tabs, actions, list, table, mapButtons, card }: {
        title: string;
        failure?: string;
        /** The props of the map. */
        items: Item[];
        shown: Item[];
        selected: string[];
        marked?: string[];
        fit: number;
        place?: { lon: number; lat: number; box?: Box };
        look: 'route' | 'ride' | 'signed';
        /** The credits that the map shows after its own: of the pictures that the page shows, of the sources of its routes. */
        credits: string[];
        /** The lines of the row of a line, its lead line first: the row has it as `data-line`. Without it, a line stands
         * alone. */
        together?: (id: string) => string[];
        onselect: (id: string | undefined) => void;
        onview: (view: Box) => void;
        /** Beside the title. */
        tabs?: Snippet;
        /** The buttons at the end of the header. */
        actions?: Snippet;
        /** The content of the list card. Each row has the id of its lead line in `data-line`. */
        list: Snippet;
        /** Under the map, with a handle that sets its height. Its rows have `data-line` too. */
        table?: Snippet;
        /** At the top right of the map, left of its zoom buttons. */
        mapButtons?: Snippet;
        /** The card of the selected item. The page gives it only while an item is selected. It gets a function that
         * moves the focus to the current row of the list: a card that closes itself calls it first, because its close
         * button goes with it. */
        card?: Snippet<[() => void]>;
    } = $props();

    /** The height of the selected card, which covers the bottom of the map. */
    let cardHeight = $state(0);
    /** The width of the buttons of the page on the map: the credits of the map end before them. */
    let buttonsWidth = $state(0);
    let listBox = $state<HTMLElement | null>(), tableBox = $state<HTMLElement | null>();
    // A box that goes is null.
    const boxes = $derived([listBox, tableBox].filter(box => !!box));
    /** The table opens at 46 % of the height of the map and the table; the map keeps at least `MAP_MIN`. */
    const TABLE_MIN = 120, MAP_MIN = 160, HANDLE = 8;
    let geoHeight = $state(0), wanted = $state<number>();
    const tableMax = $derived(Math.max(TABLE_MIN, geoHeight - HANDLE - MAP_MIN));
    const tableHeight = $derived(Math.round(Math.min(tableMax, Math.max(TABLE_MIN, wanted ?? geoHeight * 0.46))));
    const hover = new HoverState();
    /** The lead line of the lit row: another line of the same row changes nothing on the map or in the list. */
    const lead = $derived(hover.id === undefined ? undefined : (together?.(hover.id) ?? [hover.id])[0]);
    /** The focus that `toRow` moves lights nothing: after the close button the rider sees nothing lit. */
    let quiet = false;
    // With no scroll: on a phone the list is above the map, and the rider who closes the card looks at the map.
    function toRow() {
        quiet = true;
        for (const box of boxes) box.querySelector<HTMLElement>('[aria-current="true"]')?.focus({ preventScroll: true });
        quiet = false;
    }

    const lineOf = (target: EventTarget | null) => (target as Element | null)?.closest<HTMLElement>('[data-line]')?.dataset.line;
    /** A tap lights nothing: it selects. */
    const mouse = (event: PointerEvent) => event.pointerType === 'mouse' || event.pointerType === 'pen';
    /** The last place of the pointer in the list, to find the row under it when the list moves under a still pointer. */
    let pointer: { x: number; y: number } | undefined, frame = 0;
    function onpointerover(event: PointerEvent) {
        if (!mouse(event)) return;
        pointer = { x: event.clientX, y: event.clientY };
        hover.point(lineOf(event.target), 'list');
    }
    function onpointermove(event: PointerEvent) {
        if (mouse(event)) pointer = { x: event.clientX, y: event.clientY };
    }
    function onpointerleave() {
        pointer = undefined;
        hover.leave('list');
    }
    function lookup() {
        const at = pointer;
        if (at) hover.lookup(() => lineOf(document.elementFromPoint(at.x, at.y)));
    }
    function onscroll() {
        if (pointer && !frame) frame = requestAnimationFrame(() => { frame = 0; lookup(); });
    }
    // A click or a tap on a row does not match `:focus-visible`, a key does.
    function onfocusin(event: FocusEvent) {
        const row = (event.target as Element).closest<HTMLElement>('[data-line]');
        if (quiet || !row?.matches(':focus-visible')) hover.blur();
        else hover.focus(row.dataset.line!);
    }
    $effect(() => {
        void shown;
        void tick().then(lookup);
    });
    // The listeners only watch the rows: a box of rows is no control, so they are not attributes that ask for a role.
    $effect(() => {
        const stops = boxes.flatMap(box => [on(box, 'pointerover', onpointerover), on(box, 'pointermove', onpointermove), on(box, 'pointerleave', onpointerleave),
            on(box, 'scroll', onscroll, { capture: true }), on(box, 'focusin', onfocusin), on(box, 'focusout', () => hover.blur())]);
        return () => { for (const stop of stops) stop(); cancelAnimationFrame(frame); };
    });

    /** The rows that have `data-hover`. The marks change outside the state of the rows, so only those rows change. A
     * row that the list draws later gets its mark at the next change. */
    let lit: Element[] = [];
    $effect(() => {
        const line = lead;
        for (const row of lit) row.removeAttribute('data-hover');
        lit = line === undefined ? [] : boxes.flatMap(box => [...box.querySelectorAll(`[data-line="${CSS.escape(line)}"]`)]);
        for (const row of lit) row.setAttribute('data-hover', '');
    });

    // A card that is fixed at the bottom of the screen covers the end of the document: the document gets its height
    // as room there. The rule of the phone layout reads it.
    $effect(() => {
        if (!card || !cardHeight) return;
        document.body.style.setProperty('--card-room', `${cardHeight + 24}px`);
        return () => { document.body.style.removeProperty('--card-room'); };
    });
</script>

<!-- Escape closes the card as the close button does, and lights nothing either: the row that a key focused goes dark until
     the next key moves the focus. Escape in an open window closes only the window. -->
<svelte:window onkeydown={event => { if (event.key === 'Escape' && !document.querySelector('dialog[open]')) hover.blur(); }} />

<div class="routes-page">
    <header>
        <div class="title">
            <h1>{title}</h1>
            {@render tabs?.()}
        </div>
        {#if actions}<nav class="buttons" aria-label={title}>{@render actions()}</nav>{/if}
    </header>
    {#if failure}<p class="failure" role="alert">{failure}</p>{/if}

    <div class="cards" class:below={!!table}>
        <div class="list" bind:this={listBox}>
            <Card>{@render list()}</Card>
        </div>
        <!-- One element in both forms, so that a new form keeps the map. -->
        <div class="geo" bind:clientHeight={geoHeight} style:--table-height={table ? `${tableHeight}px` : null}>
            <div class="map" style:--map-buttons={mapButtons ? `${buttonsWidth + 10}px` : null}>
                {#await import('./RoutesMap.svelte') then { default: RoutesMap }}
                    <RoutesMap {items} {shown} {selected} {marked} {fit} {place} covered={card && cardHeight ? cardHeight + 12 : 0} {credits}
                        {look} {together} hovered={lead} onhover={id => id === undefined ? hover.leave('map') : hover.point(id, 'map')} {onselect} {onview} />
                {:catch error}
                    <p class="note">{error instanceof Error ? error.message : String(error)}</p>
                {/await}
                {#if mapButtons}<div class="map-buttons" bind:clientWidth={buttonsWidth}>{@render mapButtons()}</div>{/if}
                {#if card}
                    <div class="selected" bind:offsetHeight={cardHeight}>{@render card(toRow)}</div>
                {/if}
            </div>
            {#if table}
                <PanelResize value={tableHeight} min={TABLE_MIN} max={tableMax} axis="y" label="Height of the list" onResize={value => wanted = value} />
                <div class="table" bind:this={tableBox}>{@render table()}</div>
            {/if}
        </div>
    </div>
</div>

<style>
    .routes-page {
        display: flex;
        flex-direction: column;
        gap: 12px;
        height: 100%;
        padding: 14px 20px 18px;
    }
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        justify-content: space-between;
        gap: 10px 20px;
    }
    .title {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 10px 16px;
    }
    h1 {
        margin: 0;
        font: 700 25px/1.2 var(--sans);
    }
    .buttons {
        display: flex;
        gap: 8px;
    }
    .failure {
        margin: 0;
        color: var(--coral);
    }
    .cards {
        display: grid;
        flex: 1;
        grid-template-columns: clamp(380px, 100vw / 3, 480px) 1fr;
        grid-template-rows: minmax(0, 1fr);
        gap: 14px;
        min-height: 0;
    }
    .below {
        grid-template-columns: 340px 1fr;
    }
    .list > :global(section) {
        display: flex;
        flex-direction: column;
        gap: 10px;
        height: 100%;
        min-height: 0;
    }
    /* The map, and the table under it, in one panel. */
    .geo {
        display: flex;
        flex-direction: column;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
    }
    /* A size container, so the height limit of the card over it follows its height. */
    .map {
        position: relative;
        flex: 1;
        min-height: 160px;
        overflow: hidden;
        container-type: size;
        background: var(--parchment);
    }
    .table {
        display: flex;
        flex: none;
        flex-direction: column;
        height: var(--table-height);
        min-height: 0;
    }
    .note {
        margin: 32px 16px;
        color: var(--ink-soft);
    }
    .map-buttons {
        position: absolute;
        z-index: 2;
        /* Beside the zoom buttons of MapLibre: their margin of 10 px, their width of 29 px, and 10 px. */
        top: 10px;
        right: 49px;
        display: flex;
        flex-direction: column;
        align-items: flex-end;
        gap: 6px;
    }
    /* The buttons of the map are light in both themes: `--rust` is dark on them in both, `--forest` is not. */
    .map :global(.maplibregl-ctrl button:focus-visible) {
        outline: 2px solid var(--rust);
        outline-offset: -2px;
        box-shadow: none;
    }
    /* The whole card with its close button stays in the map: its long text scrolls inside it. */
    .selected {
        position: absolute;
        z-index: 2;
        right: 12px;
        bottom: 12px;
        left: 12px;
        --card-height: calc(100cqh - 24px);
    }
    @media (max-width: 1180px) {
        .below {
            grid-template-columns: 312px 1fr;
        }
    }
    @media (max-width: 760px) {
        .routes-page {
            height: auto;
            padding: 12px 16px;
        }
        .cards {
            grid-template-columns: 1fr;
            grid-template-rows: auto;
        }
        .geo > :global([role="separator"]) {
            display: none;
        }
        .table {
            height: 60vh;
        }
        .map {
            flex: none;
            height: 70vh;
            /* Some browsers give a size container layout containment, which makes the map the containing block of the
               fixed card. */
            container-type: normal;
        }
        .below .map {
            height: min(56vh, 440px);
        }
        /* The map is under the list, so a card over the map is below the screen after a tap on a row. */
        .selected {
            position: fixed;
            z-index: 20;
            bottom: max(12px, env(safe-area-inset-bottom));
            /* The card is never higher than the screen, and leaves a part of the page over it: its long text scrolls
               inside it, so its buttons stay on the screen. */
            --card-height: calc(100dvh - 96px - env(safe-area-inset-top) - env(safe-area-inset-bottom));
        }
        :global(body) {
            padding-bottom: var(--card-room, 0);
        }
    }
</style>
