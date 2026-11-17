<!-- The card of a route over the map: its picture, its name, its numbers, its profile and its buttons. The name is the one
     link of the card. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Summary } from '../../lib/library/records';
    import Icon from '../../lib/ui/Icon.svelte';
    import MiniProfile from './MiniProfile.svelte';

    let { name, href, icon, meta, numbers, profile, state, failure, picture, mark, more, buttons }: {
        name: string;
        /** With it, the name is the link that covers the card; without it, the name is a plain heading. */
        href?: string;
        /** The name of an icon of `Icon.svelte`, before the line under the name. */
        icon?: string;
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
        /** Buttons with the class `button`: `button primary` is filled, and `button quiet` is plain text. */
        buttons?: Snippet;
    } = $props();

    const metaText = $derived(meta.filter(Boolean).join(' · '));
</script>

<section class="card" aria-label={name}>
    <div class="picture">{@render picture()}</div>
    <div class="text">
        <h2>{@render mark?.()}{#if href}<a class="open" {href}>{name}</a>{:else}{name}{/if}</h2>
        {#if metaText}<p class="meta">{#if icon}<span class="icon"><Icon name={icon} size={15} /></span>{/if}{metaText}</p>{/if}
        {#if numbers.length}<p class="numbers">{#each numbers as number, i (i)}<span>{number}</span>{/each}</p>{/if}
        {#if state}<p class="state">{state}</p>{/if}
        {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
    </div>
    {#if profile}<div class="profile"><MiniProfile summary={profile} /></div>{/if}
    <div class="buttons">{@render buttons?.()}</div>
    {#if more}<div class="more">{@render more()}</div>{/if}
</section>

<style>
    /* The profile is a strip under the text and the buttons, as high as the room that they leave beside the picture. */
    .card {
        position: relative;
        display: grid;
        grid-template-columns: 300px minmax(0, 1fr) auto;
        grid-template-rows: auto minmax(40px, 1fr);
        grid-template-areas: "picture text buttons" "picture profile profile";
        align-items: start;
        gap: 10px 20px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
        box-shadow: 0 6px 20px rgb(28 27 20 / 0.18);
    }
    .picture {
        position: relative;
        grid-area: picture;
        height: 200px;
        overflow: hidden;
        border-radius: 6px;
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
    .icon {
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
        contain: size;
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
    .buttons {
        display: flex;
        grid-area: buttons;
        flex-direction: column;
        gap: 6px;
        padding-top: 6px;
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
    }
    .buttons :global(.button:not(:disabled):hover) {
        background: var(--parchment-2);
    }
    /* As strong as the rule above, so the pointer keeps the fill. */
    .buttons :global(.primary), .buttons :global(.primary:not(:disabled):hover) {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
        text-align: center;
        text-decoration: none;
    }
    /* A quiet button is plain text, so the card has one framed button. */
    .buttons :global(.quiet) {
        border-color: transparent;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .more {
        grid-column: 1 / -1;
    }
    /* Under this width the map leaves the card too little room beside the picture, so the strip goes under the card. */
    @media (max-width: 1200px) {
        .card {
            grid-template-columns: clamp(120px, 35%, 240px) minmax(0, 1fr);
            grid-template-rows: auto 1fr;
            grid-template-areas: "picture text" "picture buttons";
            gap: 10px 16px;
        }
        .picture {
            height: auto;
            aspect-ratio: 3 / 2;
        }
        .text, .buttons {
            padding-top: 0;
        }
        .buttons {
            flex-direction: row;
            flex-wrap: wrap;
            align-self: end;
        }
        .profile {
            grid-area: auto;
            grid-column: 1 / -1;
            height: 56px;
        }
    }
    @media (max-width: 760px) {
        .card {
            grid-template-columns: 120px minmax(0, 1fr);
            grid-template-areas: "picture text" "buttons buttons";
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
