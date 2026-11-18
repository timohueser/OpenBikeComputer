<!-- The card of a route over the map: its picture, its name, its numbers, its profile and its buttons. The name is the one
     link of the card. It has one width and one layout for its width in each place that shows it. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Summary } from '../../lib/library/records';
    import Icon from '../../lib/ui/Icon.svelte';
    import MiniProfile from './MiniProfile.svelte';

    let { name, href, meta, numbers, profile, state, failure, picture, mark, more, buttons, onclose }: {
        name: string;
        /** With it, the name is the link that covers the card; without it, the name is a plain heading. */
        href?: string;
        /** The parts of the line under the name. An empty list writes no line, as does an empty `numbers`. */
        meta: (string | false)[];
        numbers: string[];
        profile?: Pick<Summary, 'line' | 'heights'>;
        state?: string;
        failure?: string;
        picture: Snippet;
        /** Before the name. */
        mark?: Snippet;
        /** A second line over the full width of the card. */
        more?: Snippet;
        /** Buttons with the class `button`: `button primary` for the one action of a card, and a second one with
         * `button quiet` is plain text. */
        buttons?: Snippet;
        /** With it, the card has a button that closes it. */
        onclose?: () => void;
    } = $props();

    const metaParts = $derived(meta.filter(Boolean));
</script>

<div class="frame">
    <section class="card" aria-label={name}>
        <div class="picture">{@render picture()}</div>
        <div class="text">
            <h2>{@render mark?.()}{#if href}<a class="open" {href}>{name}</a>{:else}{name}{/if}</h2>
            {#if metaParts.length}<p class="meta">{#each metaParts as part, i (i)}{i ? ' ' : ''}<span>{part}{i < metaParts.length - 1 ? ' ·' : ''}</span>{/each}</p>{/if}
            {#if numbers.length}<p class="numbers">{#each numbers as number, i (i)}<span>{number}</span>{/each}</p>{/if}
            {#if state}<p class="state">{state}</p>{/if}
            {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
        </div>
        {#if profile}<div class="profile"><MiniProfile summary={profile} /></div>{/if}
        <div class="buttons">{@render buttons?.()}</div>
        {#if more}<div class="more">{@render more()}</div>{/if}
        {#if onclose}<button type="button" class="close" aria-label="Close" onclick={onclose}><Icon name="close" size={16} /></button>{/if}
    </section>
</div>

<style>
    /* The card changes its layout by its own width, so the same width gives the same card on the map and in a window. */
    .frame {
        max-width: 980px;
        margin: 0 auto;
        container-type: inline-size;
        transition: opacity 180ms cubic-bezier(0.23, 1, 0.32, 1), transform 180ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    @starting-style {
        .frame {
            opacity: 0;
            transform: translateY(6px);
        }
    }
    @media (prefers-reduced-motion: reduce) {
        .frame {
            transition-property: opacity;
        }
        @starting-style {
            .frame {
                transform: none;
            }
        }
    }
    .card {
        position: relative;
        display: grid;
        /* The profile takes the room that the text leaves, and keeps a fair width beside a long name. */
        grid-template-columns: 220px minmax(0, max-content) minmax(clamp(140px, 22%, 200px), 1fr) auto;
        /* A place that fixes the card to the screen limits its height with `--card-height`. The rows of the card keep
           their height; the row of `more` comes after them, takes the rest and scrolls. */
        grid-template-rows: auto;
        grid-auto-rows: minmax(0, 1fr);
        max-height: var(--card-height, none);
        align-items: center;
        gap: 20px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
        box-shadow: 0 6px 20px rgb(28 27 20 / 0.18);
    }
    .picture {
        position: relative;
        height: 130px;
        overflow: hidden;
        border-radius: 6px;
    }
    /* A long name has two lines; the page of the route has all of it. */
    h2 {
        display: -webkit-box;
        margin: 0;
        overflow: hidden;
        font: 700 18px/1.25 var(--sans);
        text-wrap: balance;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
        line-clamp: 2;
    }
    p {
        margin: 2px 0 0;
    }
    .meta, .state {
        color: var(--ink-soft);
        font-size: 13px;
    }
    /* The line wraps between two parts, and inside a part only when the part is longer than the line. */
    .meta span {
        display: inline-block;
    }
    .numbers {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 16px;
        margin-top: 8px;
        font: 600 15px/1.2 var(--sans);
        white-space: nowrap;
    }
    .state {
        margin-top: 8px;
    }
    .failure {
        color: var(--coral);
    }
    .profile {
        height: 84px;
    }
    .open {
        color: inherit;
        text-decoration: none;
    }
    /* The name is the one link of the card: it covers the whole card, under the buttons. */
    .open::after {
        position: absolute;
        inset: 0;
        border-radius: 10px;
        content: '';
    }
    .open:focus-visible {
        outline: none;
    }
    .open:focus-visible::after {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }
    .buttons, .more {
        position: relative;
        z-index: 1;
    }
    /* The last column, also on a card with no profile. */
    .buttons {
        display: flex;
        flex-direction: column;
        grid-column: 4;
        gap: 6px;
    }
    .buttons:empty {
        display: none;
    }
    .buttons :global(.button) {
        padding: 6px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        white-space: nowrap;
        transition: transform 160ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .buttons :global(.button:not(:disabled):hover) {
        background: var(--parchment-2);
    }
    .buttons :global(.button:not(:disabled):active) {
        transform: scale(0.97);
    }
    .buttons :global(.primary), .buttons :global(.primary:not(:disabled):hover) {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
    }
    .buttons :global(.primary:not(:disabled):hover) {
        filter: brightness(0.93);
    }
    /* The second button is plain text, so the card has one framed button. */
    .buttons :global(.quiet) {
        border-color: transparent;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .more {
        grid-column: 1 / -1;
        align-self: stretch;
        min-height: 0;
        overflow-y: auto;
    }
    .close {
        position: absolute;
        z-index: 2;
        top: 4px;
        right: 4px;
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        padding: 0;
        border: 0;
        border-radius: 50%;
        background: none;
        color: var(--ink-soft);
    }
    .close:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    /* A finger needs a larger area than the cross. */
    @media (pointer: coarse) {
        .close::before {
            position: absolute;
            inset: -8px;
            content: '';
        }
    }
    /* Under this width the card has too little room for four columns. */
    @container (max-width: 740px) {
        .card {
            grid-template-columns: 120px minmax(0, 1fr);
            grid-template-rows: auto auto;
            gap: 12px 16px;
        }
        /* The room of the close button. */
        .text {
            padding-right: 22px;
        }
        .picture {
            height: 80px;
        }
        .profile {
            display: none;
        }
        .buttons {
            flex-direction: row;
            grid-column: 1 / -1;
            justify-self: start;
        }
    }
</style>
