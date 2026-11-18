<!-- The card of a route over the map: its picture, its name, its numbers, its profile and its buttons. The link of the
     name covers the card under the buttons. The card has one width and one layout for its width in each place that
     shows it. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Summary } from '../../lib/library/records';
    import Icon from '../../lib/ui/Icon.svelte';
    import MiniProfile from './MiniProfile.svelte';

    let { name, href, icon, meta, numbers, profile, state, failure, picture, mark, more, buttons, onclose }: {
        name: string;
        /** With it, the name is the link that covers the card; without it, the name is a plain heading. */
        href?: string;
        /** The name of an icon of `Icon.svelte`, before the line under the name. */
        icon?: string;
        /** The parts of the line under the name, a part with `onclick` as a quiet button. An empty list writes no line, as
         * does an empty `numbers`. */
        meta: (string | false | { text: string; onclick: () => void })[];
        numbers: string[];
        profile?: Pick<Summary, 'line' | 'heights'>;
        state?: string;
        failure?: string;
        picture: Snippet;
        /** Before the name. */
        mark?: Snippet;
        /** A second line over the full width of the card. */
        more?: Snippet;
        /** Buttons with the class `button`: `button primary` is filled, and `button quiet` is plain text. */
        buttons?: Snippet;
        /** With it, the card has a button that closes it. */
        onclose?: () => void;
    } = $props();

    const metaParts = $derived(meta.filter((part): part is Exclude<typeof part, false> => !!part));
</script>

<div class="frame">
    <section class="card" aria-label={name}>
        <div class="picture">{@render picture()}</div>
        <div class="text">
            <h2>{@render mark?.()}{#if href}<a class="open" {href}>{name}</a>{:else}{name}{/if}</h2>
            {#if metaParts.length}<p class="meta">{#if icon}<span class="icon"><Icon name={icon} size={15} /></span>{/if}{#each metaParts as part, i (i)}{i ? ' ' : ''}<span>{#if typeof part === 'string'}{part}{:else}<button type="button" class="link" onclick={part.onclick}>{part.text}</button>{/if}{i < metaParts.length - 1 ? ' ·' : ''}</span>{/each}</p>{/if}
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
    /* The profile is a strip under the text and the buttons, as high as the room that they leave beside the picture. A
       place limits the height of the card with `--card-height`: the rows of the picture keep their height, and the row
       of `more` after them takes the rest and scrolls. */
    .card {
        position: relative;
        display: grid;
        grid-template-columns: 300px minmax(0, 1fr) auto;
        grid-template-rows: auto minmax(auto, 1fr);
        grid-template-areas: "picture text buttons" "picture profile profile";
        grid-auto-rows: minmax(0, auto);
        max-height: var(--card-height, none);
        align-items: start;
        gap: 10px 20px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
        box-shadow: 0 6px 20px rgb(28 27 20 / 0.18);
    }
    /* A low limit of the card's height makes the picture lower, so the card holds it: less the padding and the borders,
       and with `more` also its gap and about three lines. */
    .picture {
        position: relative;
        grid-area: picture;
        height: 200px;
        max-height: calc(var(--card-height) - 22px);
        overflow: hidden;
        border-radius: 6px;
    }
    .card:has(.more) .picture {
        max-height: calc(var(--card-height) - 88px);
    }
    .text {
        grid-area: text;
        padding-top: 6px;
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
    .link {
        position: relative;
        z-index: 1;
        padding: 0;
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        text-decoration: underline;
        text-decoration-color: var(--line-strong);
        text-underline-offset: 2px;
    }
    .link:hover {
        color: var(--ink);
    }
    .meta .icon {
        display: inline-flex;
        margin-right: 5px;
        color: var(--ink);
        vertical-align: -3px;
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
    /* The profile draws to any box, so its content gives the track no size. */
    .profile {
        grid-area: profile;
        align-self: stretch;
        min-height: 40px;
        contain: size;
    }
    .open {
        color: inherit;
        text-decoration: none;
    }
    /* The link of the name covers the whole card, under the buttons. */
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
    @media (hover: hover) {
        .open:hover {
            text-decoration: underline;
        }
        .card:has(.open:hover) {
            border-color: var(--line-strong);
        }
    }
    .buttons, .more {
        position: relative;
        z-index: 1;
    }
    /* The buttons keep the room of the close button free, also when there are none. */
    .buttons {
        display: flex;
        grid-area: buttons;
        flex-direction: column;
        gap: 6px;
        padding: 6px 26px 0 0;
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
    /* As strong as the rule of the pointer above, so the pointer keeps the fill. */
    .buttons :global(.primary), .buttons :global(.primary:not(:disabled):hover) {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
        text-align: center;
        text-decoration: none;
    }
    .buttons :global(.primary:not(:disabled):hover) {
        filter: brightness(0.93);
    }
    /* A quiet button is plain text, so the card has one framed button. */
    .buttons :global(.quiet) {
        border-color: transparent;
        color: var(--ink-soft);
        font-size: 13px;
    }
    /* Its own row, also when the card has no profile. */
    .more {
        grid-row: 3;
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
    /* Under this width the card has too little room beside the picture, so the strip goes under the card. */
    @container (max-width: 740px) {
        .card {
            grid-template-columns: clamp(120px, 35%, 240px) minmax(0, 1fr);
            grid-template-rows: auto auto auto;
            grid-template-areas: "picture text" "picture buttons" "profile profile";
            grid-auto-rows: minmax(0, 1fr);
            gap: 10px 16px;
        }
        .picture {
            height: auto;
            aspect-ratio: 3 / 2;
        }
        /* The room of the close button. */
        .text {
            padding: 0 22px 0 0;
        }
        .buttons {
            flex-direction: row;
            flex-wrap: wrap;
            align-self: end;
            padding: 0;
        }
        .buttons:empty {
            display: none;
        }
        .profile {
            height: 56px;
        }
        .more {
            grid-row: 4;
        }
        /* A low window leaves too little room for the strip and three lines of `more` in each place that shows the
           card: the text keeps the room. */
        @media (max-height: 480px) {
            .card:has(.more) .profile {
                display: none;
            }
            /* The text takes the row of the strip, so no empty row and gap stay. */
            .card:has(.more) {
                grid-template-rows: auto auto minmax(0, 1fr);
            }
            .more {
                grid-row: 3;
            }
        }
    }
    @container (max-width: 480px) {
        .card {
            grid-template-columns: 120px minmax(0, 1fr);
            grid-template-areas: "picture text" "buttons buttons" "profile profile";
            gap: 12px 16px;
        }
        .picture {
            height: 80px;
            aspect-ratio: auto;
        }
        .buttons {
            align-self: start;
            justify-self: start;
        }
    }
</style>
