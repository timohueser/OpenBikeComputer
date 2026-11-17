<!-- The frame of a page with a list beside a map: the header with the title, the list in a card, the map, and the card
     of the selected item over the map. On a phone the map is under the list. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import Card from '../../lib/ui/Card.svelte';
    import type { Box, Item } from './filter';
    import * as proto from './prototype-sizes';

    let { title, failure, items, shown, selected, fit, place, look, credits, together, onselect, onview, tabs, actions, list, card }: {
        title: string;
        failure?: string;
        /** The props of the map. */
        items: Item[];
        shown: Item[];
        selected: string[];
        fit: number;
        place?: { lon: number; lat: number; box?: Box };
        look: 'route' | 'ride';
        /** The credits of the picture of the card, which the map shows while the card is open. */
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
        /** The card of the selected item. The page gives it only while an item is selected. */
        card?: Snippet;
    } = $props();

    /** The height of the selected card, which covers the bottom of the map. */
    let cardHeight = $state(0);
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

    <div class="cards" class:wide={proto.on}>
        <div class="list">
            <Card>{@render list()}</Card>
        </div>
        <div class="map">
            {#await import('./RoutesMap.svelte') then { default: RoutesMap }}
                <RoutesMap {items} {shown} {selected} {fit} {place} covered={card && cardHeight ? cardHeight + 12 : 0} credits={card ? credits : []}
                    {look} {together} {onselect} {onview} />
            {:catch error}
                <p class="note">{error instanceof Error ? error.message : String(error)}</p>
            {/await}
            {#if card}
                <div class="selected" bind:offsetHeight={cardHeight}>{@render card()}</div>
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
    .cards.wide {
        grid-template-columns: min(480px, 33.33vw) 1fr;
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
        .cards, .cards.wide {
            grid-template-columns: 1fr;
            grid-template-rows: auto;
        }
        .map {
            height: 70vh;
        }
    }
</style>
