<!-- The card of a route over the map: its picture, its name, its numbers, its profile and its buttons. The name is the one
     link of the card. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Summary } from '../../lib/library/records';
    import Icon from '../../lib/ui/Icon.svelte';
    import MiniProfile from './MiniProfile.svelte';
    import * as proto from './prototype-sizes';

    let { name, href, meta, numbers, profile, state, failure, picture, mark, more, buttons, activity }: {
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
        /** Buttons with the class `button`; a second one with `button quiet` is plain text. */
        buttons?: Snippet;
        /** The icon name of the activity, before the first part of `meta`, which is its word. */
        activity?: string;
    } = $props();
    // The sizes of a card with a picture `card` px high, in proportion to today's 130 px.
    const k = proto.card ? proto.card / 130 : 1, t = Math.sqrt(k);
    const sizes = proto.card ? `--pic-w:${Math.round(1.5 * proto.card)}px;--pic-h:${proto.card}px;--prof-h:${Math.round(84 * k)}px;--name:${(18 * t).toFixed(1)}px;--num:${(15 * t).toFixed(1)}px;--meta:${(13 * t).toFixed(1)}px` : undefined;

    const metaText = $derived(meta.filter(Boolean).join(' · '));
</script>

<section class="card {proto.profile && `profile-${proto.profile}`}" aria-label={name} style={sizes}>
    <div class="picture">{@render picture()}</div>
    <div class="text">
        <h2>{@render mark?.()}{#if href}<a class="open" {href}>{name}</a>{:else}{name}{/if}</h2>
        {#if metaText}<p class="meta">{#if proto.on && activity}<span class="activity"><Icon name={activity} size={15} /></span>{/if}{metaText}</p>{/if}
        {#if numbers.length}<p class="numbers">{#each numbers as number, i (i)}<span>{number}</span>{/each}</p>{/if}
        {#if state}<p class="state">{state}</p>{/if}
        {#if failure}<p class="failure" role="alert">{failure}</p>{/if}
    </div>
    {#if profile}<div class="profile"><MiniProfile summary={profile} /></div>{/if}
    <div class="buttons">{@render buttons?.()}</div>
    {#if more}<div class="more">{@render more()}</div>{/if}
</section>

<style>
    .card {
        position: relative;
        display: grid;
        /* The profile takes the room that the text leaves, and keeps a fair width beside a long name. */
        grid-template-columns: var(--pic-w, 220px) minmax(0, max-content) minmax(clamp(140px, 22%, 200px), 1fr) auto;
        align-items: center;
        gap: 20px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
        box-shadow: 0 6px 20px rgb(28 27 20 / 0.18);
    }
    /* The prototype keeps the text at least 170 px wide; the profile gives up room first. */
    .card[style] {
        grid-template-columns: var(--pic-w) minmax(170px, max-content) minmax(100px, 1fr) auto;
    }
    .picture {
        position: relative;
        height: var(--pic-h, 130px);
        overflow: hidden;
        border-radius: 6px;
    }
    /* A long name has two lines; the page of the route has all of it. */
    h2 {
        display: -webkit-box;
        margin: 0;
        overflow: hidden;
        font: 700 var(--name, 18px)/1.25 var(--sans);
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
        line-clamp: 2;
    }
    p {
        margin: 2px 0 0;
    }
    .meta, .state {
        color: var(--ink-soft);
        font-size: var(--meta, 13px);
    }
    .activity {
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
        font: 600 var(--num, 15px)/1.2 var(--sans);
        white-space: nowrap;
    }
    .state {
        margin-top: 8px;
    }
    .failure {
        color: var(--coral);
    }
    .profile {
        height: var(--prof-h, 84px);
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
    .buttons {
        display: flex;
        flex-direction: column;
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
    }
    .buttons :global(.button:not(:disabled):hover) {
        background: var(--parchment-2);
    }
    .buttons :global(.button.primary), .buttons :global(.button.primary:hover) {
        border-color: var(--rust);
        background: var(--rust);
        color: var(--cream);
        text-align: center;
        text-decoration: none;
    }
    /* The second button is plain text, so the card has one framed button. */
    .buttons :global(.quiet) {
        border-color: transparent;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .more {
        grid-column: 1 / -1;
    }
    /* Under this width the map leaves the card too little room for four columns. */
    /* The profile as a wide strip: right of the picture under the text and the buttons, or under the whole card. */
    .card.profile-strip, .card.profile-under, .card.profile-none {
        grid-template-columns: var(--pic-w) minmax(0, 1fr) auto;
        grid-template-areas: "pic text buttons" "pic prof prof";
        grid-template-rows: 1fr 72px;
        align-items: start;
        gap: 10px 20px;
    }
    .card.profile-under {
        grid-template-areas: "pic text buttons" "prof prof prof";
        grid-template-rows: auto 56px;
    }
    .card.profile-none {
        grid-template-areas: "pic text buttons";
        grid-template-rows: auto;
        align-items: center;
    }
    .card[class*="profile-"] .picture { grid-area: pic; }
    .card[class*="profile-"] .text { grid-area: text; padding-top: 6px; }
    .card[class*="profile-"] .buttons { grid-area: buttons; padding-top: 6px; }
    .card[class*="profile-"] .profile { grid-area: prof; align-self: stretch; height: auto; }
    .card.profile-none .profile { display: none; }
    .card.profile-none .text, .card.profile-none .buttons { padding-top: 0; }
    @media (max-width: 1200px) {
        .card[style].profile-strip, .card[style].profile-under, .card[style].profile-none {
            grid-template-columns: calc(0.8 * var(--pic-w)) minmax(0, 1fr);
            /* Beside a narrow map, text and buttons fill the side of the picture; the strip goes under both. */
            grid-template-areas: "pic text" "pic buttons" "prof prof";
            grid-template-rows: auto 1fr 56px;
        }
        .card[style].profile-under {
            grid-template-areas: "pic text" "pic buttons" "prof prof";
            grid-template-rows: auto 1fr 48px;
        }
        .card[style].profile-none {
            grid-template-areas: "pic text" "pic buttons";
            grid-template-rows: auto 1fr;
        }
        .card[class*="profile-"] .picture {
            height: calc(0.8 * var(--pic-h));
        }
        .card[class*="profile-"] .profile {
            display: block;
        }
        .card[style].profile-none .profile {
            display: none;
        }
        .card[class*="profile-"] .buttons {
            grid-column: auto;
            padding-top: 0;
        }
    }
    @media (max-width: 1200px) {
        .card {
            grid-template-columns: 120px minmax(0, 1fr);
            gap: 12px 16px;
        }
        .card[style] {
            grid-template-columns: calc(0.6 * var(--pic-w)) minmax(0, 1fr);
        }
        .picture {
            height: calc(0.62 * var(--pic-h, 130px));
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
