<!-- The frame of a page with a list beside a map: the header with the title, the list in a card, the map, and the card
     of the selected item over the map. On a phone the map is under the list, and the card is at the bottom of the screen. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import Card from '../../lib/ui/Card.svelte';
    import type { Box, Item } from './filter';

    let { title, failure, items, shown, selected, marked, fit, place, look, credits, together, onselect, onview, tabs, actions, list, mapButtons, card }: {
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
        /** The credits that the map shows after its own: of the picture of the open card, of the sources of its routes. */
        credits: string[];
        /** The lines that a click on one line selects, for the map. Without it, a line stands alone. */
        together?: (id: string) => string[];
        onselect: (id: string | undefined) => void;
        onview: (view: Box) => void;
        /** Beside the title. */
        tabs?: Snippet;
        /** The buttons at the end of the header. */
        actions?: Snippet;
        /** The content of the list card. */
        list: Snippet;
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
    let listBox = $state<HTMLElement>();
    // With no scroll: on a phone the list is above the map, and the rider who closes the card looks at the map.
    const toRow = () => listBox?.querySelector<HTMLElement>('[aria-current="true"]')?.focus({ preventScroll: true });

    // A card that is fixed at the bottom of the screen covers the end of the document: the document gets its height
    // as room there. The rule of the phone layout reads it.
    $effect(() => {
        if (!card || !cardHeight) return;
        document.body.style.setProperty('--card-room', `${cardHeight + 24}px`);
        return () => { document.body.style.removeProperty('--card-room'); };
    });
</script>

<div class="routes-page">
    <header>
        <div class="title">
            <h1>{title}</h1>
            {@render tabs?.()}
        </div>
        {#if actions}<nav class="buttons" aria-label={title}>{@render actions()}</nav>{/if}
    </header>
    {#if failure}<p class="failure" role="alert">{failure}</p>{/if}

    <div class="cards">
        <div class="list" bind:this={listBox}>
            <Card>{@render list()}</Card>
        </div>
        <div class="map" style:--map-buttons={mapButtons ? `${buttonsWidth + 10}px` : null}>
            {#await import('./RoutesMap.svelte') then { default: RoutesMap }}
                <RoutesMap {items} {shown} {selected} {marked} {fit} {place} covered={card && cardHeight ? cardHeight + 12 : 0} {credits}
                    {look} {together} {onselect} {onview} />
            {:catch error}
                <p class="note">{error instanceof Error ? error.message : String(error)}</p>
            {/await}
            {#if mapButtons}<div class="map-buttons" bind:clientWidth={buttonsWidth}>{@render mapButtons()}</div>{/if}
            {#if card}
                <div class="selected" bind:offsetHeight={cardHeight}>{@render card(toRow)}</div>
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
        grid-template-columns: 380px 1fr;
        grid-template-rows: minmax(0, 1fr);
        gap: 14px;
        min-height: 0;
    }
    .list > :global(section) {
        display: flex;
        flex-direction: column;
        gap: 10px;
        height: 100%;
        min-height: 0;
    }
    .map {
        position: relative;
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--parchment);
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
    .selected {
        position: absolute;
        z-index: 2;
        right: 12px;
        bottom: 12px;
        left: 12px;
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
        .map {
            height: 70vh;
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
