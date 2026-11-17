<!-- A row of a list of routes: a small picture, the name, a line of numbers, and a badge. The row shows its picture only
     while it is near the view, so a long list draws few pictures. -->
<script module lang="ts">
    const rows = new WeakMap<Element, (near: boolean) => void>();
    let observer: IntersectionObserver | undefined;
</script>

<script lang="ts">
    import type { Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import * as proto from './prototype-sizes';

    let { name, current, onclick, lead = '', parts, tail, badge, room, picture, activity }: {
        name: string;
        current: boolean;
        onclick: () => void;
        /** The second line, written when `parts` is given: `lead` (such as a date), each part in its own span, so the
         * line wraps between two parts and never inside one, then `tail` as plain text. */
        lead?: string;
        parts?: (string | false)[];
        tail?: string;
        badge?: { text: string; kind: string };
        /** The text of a button that lies on the row. The row keeps its room with the same text, hidden, where a badge
         * would be. */
        room?: string;
        /** A cover, whose root has the class `cover`. */
        picture: Snippet;
        /** The icon name of the activity. */
        activity?: string;
    } = $props();

    let near = $state(false);
    function watch(node: Element) {
        observer ??= new IntersectionObserver(entries => {
            for (const { target, isIntersecting } of entries) rows.get(target)?.(isIntersecting);
        }, { rootMargin: '200px' });
        rows.set(node, value => near = value);
        observer.observe(node);
        return { destroy: () => observer!.unobserve(node) };
    }
</script>

<button type="button" class="row" class:row1={proto.row === 1} class:row2={proto.row === 2} aria-current={current} {onclick} use:watch>
    <span class="thumb">{#if near}{@render picture()}{/if}</span>
    <span class="text">
        <strong>{#if proto.on && activity}<span class="activity"><Icon name={activity} size={16} /></span>{/if}{name}</strong>
        {#if parts}
            <span class="meta">{lead}{#each parts.filter(Boolean) as part, i (i)}{i || lead ? ' · ' : ''}<span>{part}</span>{/each}{#if tail}{' · '}{tail}{/if}</span>
        {/if}
    </span>
    {#if badge}<span class="badge {badge.kind}">{badge.text}</span>{:else if room}<span class="room" aria-hidden="true">{room}</span>{/if}
</button>

<style>
    .row {
        display: grid;
        grid-template-columns: 72px minmax(0, 1fr) auto;
        align-items: center;
        gap: 12px;
        width: 100%;
        padding: 6px;
        border: 0;
        border-radius: 8px;
        background: none;
        color: var(--ink);
        text-align: left;
    }
    .row:hover {
        background: var(--parchment);
    }
    .row[aria-current="true"] {
        background: var(--parchment-2);
    }
    .thumb {
        height: 48px;
        overflow: hidden;
        border-radius: 6px;
        background: var(--parchment-2);
    }
    .thumb :global(.cover) {
        background: none;
    }
    .text {
        display: flex;
        flex-direction: column;
        gap: 1px;
        min-width: 0;
    }
    /* The numbers are one line where they fit and wrap where they do not: none is cut. */
    .meta, .badge, .room {
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .badge, .meta span, .room {
        white-space: nowrap;
    }
    /* The one state that waits for the rider to connect the device has a dot. */
    .badge.waiting::before {
        display: inline-block;
        width: 6px;
        height: 6px;
        margin-right: 5px;
        border-radius: 50%;
        background: var(--amber);
        vertical-align: 1px;
        content: '';
    }
    /* The padding of the button that lies on the row. */
    .room {
        padding: 2px 4px;
        visibility: hidden;
    }
    .activity {
        display: inline-flex;
        margin-right: 5px;
        vertical-align: -2px;
    }
    .row1 {
        grid-template-columns: 150px minmax(0, 1fr) auto;
        align-items: start;
        gap: 14px;
        padding: 7px;
    }
    .row1 .thumb {
        height: 100px;
    }
    .row1 .text {
        padding-top: 6px;
        gap: 3px;
    }
    .row2 {
        grid-template-columns: minmax(0, 1fr) auto;
        gap: 6px 12px;
        padding: 7px;
    }
    .row2 .thumb {
        grid-column: 1 / -1;
        height: 140px;
    }
    @media (max-width: 760px) {
        .row {
            grid-template-columns: 64px minmax(0, 1fr) auto;
            gap: 10px;
        }
        .thumb {
            height: 44px;
        }
        .row1 {
            grid-template-columns: 120px minmax(0, 1fr) auto;
        }
        .row1 .thumb {
            height: 80px;
        }
        .row2 .thumb {
            height: 160px;
        }
    }
</style>
