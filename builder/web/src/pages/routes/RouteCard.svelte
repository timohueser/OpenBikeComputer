<!-- The card of a route over the map: its picture, its name, its numbers, its profile and its buttons. The name is the one
     link of the card. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Summary } from '../../lib/library/records';
    import MiniProfile from './MiniProfile.svelte';

    let { name, href, meta, numbers, profile, state, failure, picture, mark, more, buttons }: {
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
    } = $props();

    const metaText = $derived(meta.filter(Boolean).join(' · '));
</script>

<section class="card" aria-label={name}>
    <div class="picture">{@render picture()}</div>
    <div class="text">
        <h2>{@render mark?.()}{#if href}<a class="open" {href}>{name}</a>{:else}{name}{/if}</h2>
        {#if metaText}<p class="meta">{metaText}</p>{/if}
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
        grid-template-columns: 220px minmax(0, max-content) minmax(clamp(140px, 22%, 200px), 1fr) auto;
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
    @media (max-width: 1200px) {
        .card {
            grid-template-columns: 120px minmax(0, 1fr);
            gap: 12px 16px;
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
