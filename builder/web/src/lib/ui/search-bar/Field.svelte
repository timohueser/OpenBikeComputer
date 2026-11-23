<!-- The text field of the search bar and its list under the bar. While the rider types, the field marks each phrase that
     the table of the page knows, shows the glue words faint and underlines the unknown words. Enter sends each marked
     phrase to its slot, and the unknown words stay; a page that reads the text itself at Enter gives the words that
     stay. The list suggests values for the words in progress and places of the server for a typed name, and its first
     row says which marked value goes to which slot; with an empty field, it offers the values often added next, the
     last searches or examples. -->
<script lang="ts" module>
    import type { Table, Value } from './read';

    /** A place of the server: its name, and what and where it is. */
    export interface Place { name: string; about: string }

    /** What the page gives the text field. */
    export interface FieldProps<V extends Value, P extends Place> {
        table: Table<V>;
        label: string;
        placeholder: string;
        /** Sentences for an empty field before the first search: a click types one and presses Enter. */
        examples: string[];
        /** The last searches, the newest first, and what brings one back. */
        recent: { text: string; run: () => void }[];
        /** The values that riders often add next, for an empty field after Enter. */
        often: () => V[];
        /** Whether a value changes the search: the list offers no other value. */
        changes: (value: V) => boolean;
        /** Sends values to the search, in the order of the text. */
        onvalues: (values: V[]) => void;
        /** Reads the whole text at Enter in place of the marks, such as on a server, and applies what it understood. It
         * answers the words that it did not understand, which stay in the field, and the values that went to their
         * slots; undefined when a newer search replaced it. Until Enter, no word is unknown, as the page can read any. */
        read?: (text: string) => Promise<{ kept: string; values: Value[] } | undefined>;
        /** The places of the server for a typed name. Enter never takes one; a click goes to `onplace`. A new function
         * is a new search, such as that of a new map view: the field asks again. */
        places?: (name: string, signal: AbortSignal) => Promise<P[]>;
        onplace?: (place: P) => void;
        /** The words over the places in the list, which say what a click does. */
        placesLabel?: string;
        /** Enter ended a search, or a panel closed: the page can keep the search. */
        onsearch: () => void;
    }
</script>

<script lang="ts" generics="V extends Value, P extends Place">
    import { flushSync, untrack } from 'svelte';
    import Icon from '../Icon.svelte';
    import { drop, pop, still } from './motion';
    import { fold, scan, suggest, typing, type Part } from './read';

    let { table, label, placeholder, examples, recent, often, changes, onvalues, read, places, onplace, placesLabel, onsearch, text = $bindable(), listing = $bindable(), slots, land }: FieldProps<V, P> & {
        text: string;
        /** The list is open. */
        listing: boolean;
        /** The slots of the bar, in their order. */
        slots: { id: string; icon: string; label: string }[];
        /** The words of a value fly from `from` to its slot after `delay` ms. */
        land: (value: Value, from: DOMRect | undefined, delay: number) => void;
    } = $props();

    /** A row of the list. Enter takes a row with `value` in place of the words in progress, with the rest of the text.
     * `to` is the row of the marked values, by slot; `about` is some quiet words after the name of a place. */
    type Row = { icon: string; path?: string; text: string; typed?: number; side?: string; about?: string; to?: typeof reading; value?: V; pick: (from?: DOMRect) => void };
    type Line = { text: string; remove?: Part<V>[] };

    const WAIT_MS = 300, KEY_MS = 48, SPACE_MS = 120;
    let input = $state<HTMLInputElement>(), mirror = $state<HTMLElement>(), listbox = $state<HTMLElement>();
    /** The places of the server, and the name and the search of the page that they answer. */
    let active = $state(-1), found = $state.raw<P[]>([]), answered = $state.raw<{ words: string; by: typeof places }>();
    /** At Enter the words in progress are done too, so the marks cover the whole text. */
    let sealed = $state(false);
    /** A value went to its slot since the list opened: an empty field then offers the values often added next. */
    let session = $state(false);
    /** The words that `read` did not understand at the last Enter. */
    let kept = $state('');

    const start = $derived(sealed ? text.length : typing(text, table));
    const fragment = $derived(text.slice(start).trim());
    const parts = $derived(scan(text.slice(0, start), table));
    /** The text in pieces, each part with its mark. */
    const pieces = $derived.by(() => {
        const out: { text: string; kind?: Part<V>['kind']; at?: number }[] = [];
        let at = 0;
        parts.forEach((part, i) => {
            out.push({ text: text.slice(at, part.from) }, { text: text.slice(part.from, part.to), kind: quiet(part) ? undefined : part.kind, at: i });
            at = part.to;
        });
        return [...out, { text: text.slice(at) }];
    });
    /** The words at the end that no phrase knows: a name, which the server can find as a place. */
    const name = $derived.by(() => {
        if (!places) return { from: text.length, words: '' };
        const all = scan(text, table);
        let from = text.length;
        for (let i = all.length - 1; i >= 0; i--) {
            const part = all[i];
            if (part.kind !== 'unknown' || part.note) break;
            from = part.from;
        }
        const words = text.slice(from).trim();
        return { from, words: listing && /\p{L}.*\p{L}/u.test(words) && !/\d/.test(words) ? words : '' };
    });

    /** The answer for the name, in the search of the page, is still on its way. */
    const asking = $derived(!!name.words && (answered?.words !== name.words || answered.by !== places));
    const begins = (place: P) => fold(place.name).startsWith(fold(name.words));
    /** The places in the list. Until the answer comes, the places before whose name begins with the new name stay, so
     * the rows do not blink while the rider types. */
    const offered = $derived(!name.words ? [] : asking ? found.filter(begins) : found);
    /** The name is not unknown while the server looks for it, or when it begins the name of a place in the list. */
    const placed = $derived(!!name.words && (asking || offered.some(begins)));
    /** A word of the name, which no phrase knows. */
    const plain = (part: Part<V>) => part.kind === 'unknown' && !part.note && part.from >= name.from;
    /** An unknown word that is not marked: a name that the server looks for, or a word that `read` can still read. */
    const quiet = (part: Part<V>) => (plain(part) && placed) || (!!read && part.kind === 'unknown' && !part.note && text !== kept);

    /** The marked values of each slot, in the order of the bar: what the first row of the list sends, and where. */
    const reading = $derived(slots.map(slot => ({ slot, labels: parts.flatMap(part => part.kind === 'value' && part.value.slot === slot.id ? [part.value.label] : []) }))
        .filter(group => group.labels.length));

    /** The lines about the unknown words: their notes, and the words that no phrase knows. */
    function lines(all: Part<V>[]): Line[] {
        const unknown = all.flatMap(part => part.kind === 'unknown' ? [part] : []);
        const lost = unknown.filter(part => !part.note && !quiet(part));
        const notes = [...new Set(unknown.flatMap(part => part.note ? [part.note] : []))].map(note => ({ text: note }));
        const words = lost.map(part => `“${text.slice(part.from, part.to)}”`).join(', ');
        return words ? [...notes, { text: `Not understood: ${words}. It changes nothing.`, remove: lost }] : notes;
    }
    const valueRow = (value: V, pick: (from?: DOMRect) => void): Row => {
        const typed = fold(value.label).startsWith(fold(fragment)) ? fragment.length : 0;
        return { icon: value.icon, path: value.path, text: value.label, typed, side: slots.find(slot => slot.id === value.slot)?.label, value, pick };
    };
    /** The rows in their order: the marked values, the suggestions, then `places` rows of places. */
    const list = $derived.by((): { title?: string; rows: Row[]; places: number; lines: Line[]; keys?: boolean } => {
        // A name can end in a space, as a phone keyboard writes it: its places show without words in progress too.
        const named = offered.map((place): Row => ({ icon: 'pin', text: place.name, about: place.about, pick: () => choose(place) }));
        // The first row sends the marked phrases alone, so the words in progress stay.
        const marked: Row[] = reading.length ? [{ icon: 'arrow', text: reading.map(({ slot, labels }) => `${slot.label}: ${labels.join(', ')}`).join('. '), to: reading,
            pick: () => commit(undefined, false) }] : [];
        if (fragment) {
            const rows = [...marked, ...suggest(fragment, table).filter(changes).map(value => valueRow(value, from => take(value, from))), ...named];
            // Short words in progress, and words with a suggestion or a place, can still become one.
            const open = rows.length > marked.length || fragment.length < 3;
            return { rows, places: named.length, lines: lines(open ? parts : scan(text, table)), keys: rows.some(row => row.value) };
        }
        if (text.trim()) return { rows: [...marked, ...named], places: named.length, lines: lines(parts) };
        const added = session ? often().map(value => valueRow(value, from => send([[value, from]]))) : [];
        if (added.length) return { title: 'Add', rows: added, places: 0, lines: [] };
        if (recent.length) return { title: 'Last searches', rows: recent.map(one => ({ icon: 'clock', text: one.text, pick: () => { one.run(); listing = false; } })), places: 0, lines: [] };
        return { title: 'Say it in one go, for example', rows: examples.map(sentence => ({ icon: 'arrow', text: sentence, pick: () => play(sentence) })), places: 0, lines: [] };
    });
    const shown = $derived(listing && (list.rows.length > 0 || list.lines.length > 0));

    // A new text marks the first suggestion, which Enter takes for the words in progress.
    $effect.pre(() => {
        void [text, listing];
        untrack(() => active = fragment ? list.rows.findIndex(row => row.value) : -1);
    });
    $effect(() => {
        if (active >= 0) listbox?.querySelector(`#search-option-${active}`)?.scrollIntoView({ block: 'nearest' });
    });
    // The mirror scrolls with the field.
    $effect(() => {
        void pieces;
        if (mirror && input) mirror.scrollLeft = input.scrollLeft;
    });
    // The places of the server for a new name or a new search of the page, 300 ms after the last key; a newer one stops
    // the request.
    $effect(() => {
        const asked = name.words, by = places;
        if (!asked || !by) [found, answered] = [[], undefined];
        if (!by || !untrack(() => asking)) return;
        const controller = new AbortController();
        const answer = (list: P[]) => { if (!controller.signal.aborted) [found, answered] = [list, { words: asked, by }]; };
        const timer = setTimeout(() => by(asked, controller.signal).then(answer, () => answer([])), WAIT_MS);
        return () => {
            clearTimeout(timer);
            controller.abort();
        };
    });

    const optionRect = (i: number) => listbox?.querySelector(`#search-option-${i}`)?.getBoundingClientRect();

    function send(values: [V, DOMRect | undefined][]) {
        if (!values.length) return;
        onvalues(values.map(([value]) => value));
        values.forEach(([value, from], i) => land(value, from, i * 70));
        session = true;
    }
    /** Tab or a click: the value takes the place of the words in progress, and the other words stay. */
    function take(value: V, from?: DOMRect) {
        text = text.slice(0, start);
        send([[value, from]]);
    }
    /** Enter: each marked phrase goes to its slot, and the unknown words stay. `last` takes the place of the words in
     * progress, so all values go in one call, in the order of the text. Without `whole`, the words in progress are not
     * read: they stay too. With `read`, the page reads the whole text; the words of `last` complete the words in progress
     * only when they begin with them, as "Drinking water" does "drinking wat": "Day 1" for the "1" of "day 1" would repeat
     * a word. */
    function commit(last?: [V, DOMRect | undefined], whole = true) {
        if (read) return void ask(last && fold(last[0].label).startsWith(fold(fragment)) ? text.trimEnd() + last[0].label.slice(fragment.length) : text);
        if (last) text = text.slice(0, start);
        sealed = whole;
        flushSync();
        const values = parts.flatMap((part, i): [V, DOMRect | undefined][] => part.kind === 'value' ? [[part.value, mirror?.querySelector(`[data-part="${i}"]`)?.getBoundingClientRect()]] : []);
        if (last) values.push(last);
        const kept = parts.flatMap(part => part.kind === 'unknown' ? [text.slice(part.from, part.to)] : []);
        text = [...kept, text.slice(start).trim()].join(' ').trimStart();
        sealed = false;
        send(values);
        onsearch();
    }
    /** The page reads `sentence`. The words of each marked slot fly from where they were, and the words that the page
     * did not understand stay, unless the rider changed the text meanwhile. */
    async function ask(sentence: string) {
        if (!sentence.trim()) return onsearch();
        [text, sealed] = [sentence, true];
        flushSync();
        const from = new Map(parts.flatMap((part, i) => part.kind === 'value' ? [[part.value.slot, mirror?.querySelector(`[data-part="${i}"]`)?.getBoundingClientRect()] as const] : []).reverse());
        sealed = false;
        const answer = await read!(sentence);
        if (!answer) return;
        if (text === sentence) text = kept = answer.kept;
        answer.values.forEach((value, i) => land(value, from.get(value.slot), i * 70));
        session = true;
        onsearch();
    }
    function enter() {
        const row = list.rows[active];
        if (row?.value && fragment) return commit([row.value, optionRect(active)]);
        row?.pick(optionRect(active));
        if (!row || row.value) commit();
    }
    function choose(place: P) {
        text = text.slice(0, name.from);
        listing = false;
        onplace?.(place);
    }
    function remove(spans: Part<V>[]) {
        let next = text;
        for (const span of [...spans].reverse()) next = next.slice(0, span.from) + next.slice(span.to);
        text = next.replace(/\s{2,}/g, ' ').trimStart();
    }

    /** An example types itself and presses Enter; with reduced motion it is there at once. A key or a new text stops it. */
    let playing = 0;
    const stop = () => playing++;
    function play(sentence: string) {
        const run = ++playing;
        text = '';
        if (still()) {
            text = sentence;
            return commit();
        }
        let typed = 0;
        const step = () => {
            if (run !== playing || !listing) return;
            if (typed < sentence.length) {
                text += sentence[typed++];
                setTimeout(step, sentence[typed - 1] === ' ' ? SPACE_MS : KEY_MS);
            } else setTimeout(() => { if (run === playing && listing) commit(); }, 2 * SPACE_MS);
        };
        setTimeout(step, 2 * SPACE_MS);
    }
    $effect(() => stop);

    function keydown(event: KeyboardEvent) {
        stop();
        const count = list.rows.length;
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            if (!listing) listing = true;
            else if (count) active = event.key === 'ArrowDown' ? (active + 1) % count : active <= 0 ? count - 1 : active - 1;
        } else if (event.key === 'Enter') {
            event.preventDefault();
            enter();
        } else if (event.key === 'Tab' && !event.shiftKey && list.rows[active]) {
            event.preventDefault();
            list.rows[active].pick(optionRect(active));
        } else if (event.key === 'Escape' && listing) {
            event.preventDefault();
            listing = false;
        }
    }
    /** The key `/` puts the cursor in the field, unless the rider types in another field. */
    function slash(event: KeyboardEvent) {
        const at = document.activeElement as HTMLElement | null;
        if (event.key !== '/' || event.defaultPrevented || event.ctrlKey || event.metaKey || at?.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(at?.tagName ?? '')
            || document.querySelector('dialog[open]')) return;
        event.preventDefault();
        input?.focus();
    }
</script>

<svelte:window onkeydown={slash} />

<div class="field">
    <label class="say">
        <Icon name="search" size={17} />
        <span class="type">
            <span class="mirror" aria-hidden="true" bind:this={mirror}>{#each pieces as piece, i (i)}{#if piece.kind}<span class="m-{piece.kind}" data-part={piece.at}>{piece.text}</span>{:else}{piece.text}{/if}{/each}</span>
            <input bind:this={input} bind:value={text} type="text" role="combobox" aria-label={label} {placeholder} aria-expanded={shown && list.rows.length > 0} aria-controls="search-list"
                aria-autocomplete="list" aria-activedescendant={active >= 0 ? `search-option-${active}` : undefined} autocomplete="off" autocapitalize="off" spellcheck="false" enterkeyhint="go"
                onfocus={() => { if (!listing) [listing, session] = [true, false]; }} onblur={() => listing = false} onclick={() => listing = true}
                oninput={() => { stop(); listing = true; }} onkeydown={keydown} onscroll={() => { if (mirror && input) mirror.scrollLeft = input.scrollLeft; }} />
        </span>
    </label>
    {#if text.trim()}
        <button type="button" class="go" aria-label="Put the words in the slots" title="Enter" in:pop onpointerdown={event => event.preventDefault()} onclick={() => { stop(); enter(); }}>
            <Icon name="arrow" size={15} />
        </button>
    {/if}
</div>

{#snippet option(row: Row, i: number)}
    <button type="button" class="row" class:reading={!!row.to} class:active={i === active} role="option" id="search-option-{i}" tabindex="-1" aria-selected={i === active}
        aria-label={row.to ? row.text : undefined} onclick={event => { stop(); row.pick(event.currentTarget.getBoundingClientRect()); }}>
        {#if row.to}
            {#each row.to as { slot, labels } (slot.id)}
                <span class="to"><span class="slot"><Icon name={slot.icon} size={13} />{slot.label}</span><b>{labels.join(' · ')}</b></span>
            {/each}
            <!-- Enter does what this row does only with no words in progress and no other row chosen. -->
            {#if !fragment && active <= 0}<kbd>Enter</kbd>{/if}
        {:else}
            <Icon name={row.icon} path={row.path} size={16} />
            <span class="what">{#if row.typed}{row.text.slice(0, row.typed)}<b>{row.text.slice(row.typed)}</b>{:else}{row.text}{/if}{#if row.about}<i>{row.about}</i>{/if}</span>
            {#if row.side}<small>{row.side}</small>{/if}
        {/if}
    </button>
{/snippet}

{#if shown}
    <!-- A press in the list keeps the cursor in the field. -->
    <div class="search-panel list" role="presentation" in:drop out:drop={{ leave: true }} onpointerdown={event => event.preventDefault()}>
        {#if list.title}<h3>{list.title}</h3>{/if}
        {#if list.rows.length}
            {@const cut = list.rows.length - list.places}
            <div class="rows" role="listbox" id="search-list" aria-label={list.title ?? 'Suggestions'} bind:this={listbox}>
                {#each list.rows.slice(0, cut) as row, i (i)}{@render option(row, i)}{/each}
                {#if list.places}
                    <div class="places" role="group" aria-label={placesLabel}>
                        <span class="head" aria-hidden="true">{placesLabel}</span>
                        {#each list.rows.slice(cut) as row, i (i)}{@render option(row, cut + i)}{/each}
                    </div>
                {/if}
            </div>
        {/if}
        {#each list.lines as line (line.text)}
            <p class="note">{line.text}{#if line.remove}<button type="button" onclick={() => remove(line.remove!)}>Remove</button>{/if}</p>
        {/each}
        {#if list.keys}
            <p class="keys"><span><kbd>Enter</kbd> all to the slots</span><span><kbd>Tab</kbd> only this one</span><span><kbd>↑</kbd> <kbd>↓</kbd> choose</span></p>
        {/if}
    </div>
{/if}

<style>
    .field {
        display: flex;
        flex: 0 1 348px;
        align-items: center;
        gap: 6px;
        min-width: 0;
        padding: 0 10px 0 14px;
    }
    .field:has(input:focus-visible) {
        box-shadow: inset 0 -2px 0 var(--forest);
    }
    .say {
        display: flex;
        flex: 1;
        align-items: center;
        gap: 9px;
        min-width: 0;
        color: var(--ink-soft);
        cursor: text;
    }
    .type {
        position: relative;
        flex: 1;
        min-width: 0;
        height: 28px;
    }
    /* The field draws only the caret; the mirror under it draws the text with its marks. */
    .mirror, input {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 28px;
        margin: 0;
        padding: 0 2px;
        border: 0;
        font: 15px/28px var(--sans);
        letter-spacing: 0;
        white-space: pre;
    }
    .mirror {
        overflow: hidden;
        color: var(--ink);
        pointer-events: none;
    }
    input {
        outline: 0;
        background: transparent;
        color: transparent;
        caret-color: var(--rust);
    }
    input:focus-visible {
        outline: 0;
    }
    input::placeholder {
        color: var(--ink-faint);
        opacity: 1;
    }
    input::selection {
        background: color-mix(in srgb, var(--parchment-3) 75%, transparent);
    }
    .m-value {
        border-radius: 4px;
        background: var(--parchment-3);
        box-shadow: 0 0 0 1px var(--parchment-3);
        animation: seen 200ms ease-out;
    }
    @keyframes seen {
        from { background: transparent; box-shadow: 0 0 0 1px transparent; }
    }
    .m-glue {
        color: var(--ink-faint);
    }
    .m-unknown {
        text-decoration: underline dotted var(--coral);
        text-decoration-thickness: 1.5px;
        text-underline-offset: 4px;
    }
    .go {
        display: grid;
        flex: none;
        place-items: center;
        width: 28px;
        height: 28px;
        padding: 0;
        border: 0;
        border-radius: 50%;
        background: var(--amber);
        color: var(--on-amber);
        transition: background 120ms, transform 140ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .go:hover {
        background: color-mix(in srgb, var(--amber) 86%, var(--ink));
    }
    .go:active {
        transform: scale(0.94);
    }
    .list {
        left: 0;
        width: min(420px, 100%);
    }
    .to {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        min-width: 0;
    }
    .slot {
        display: inline-flex;
        flex: none;
        align-items: center;
        gap: 4px;
        color: var(--ink-soft);
        font-size: 12px;
    }
    .reading b {
        font-weight: 600;
    }
    .reading kbd {
        display: none;
        margin-left: auto;
    }
    h3, .head {
        margin: 10px 12px 2px;
        color: var(--ink-soft);
        font: 400 12px var(--sans);
    }
    .places {
        display: flex;
        flex-direction: column;
    }
    .head {
        margin: 8px 8px 2px;
    }
    .row:not(.reading) + .places {
        margin-top: 4px;
        border-top: 1px solid var(--line);
    }
    .rows {
        display: flex;
        flex-direction: column;
        padding: 4px;
    }
    .row {
        display: flex;
        align-items: center;
        gap: 9px;
        width: 100%;
        min-height: 38px;
        padding: 6px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
        font: 14px var(--sans);
        text-align: left;
    }
    .row > :global(svg) {
        flex: none;
        color: var(--ink-soft);
    }
    /* The marked values under the name of their slot, from edge to edge of the list. */
    .reading {
        flex-wrap: wrap;
        gap: 4px 16px;
        width: auto;
        min-height: 42px;
        margin: -4px -4px 4px;
        padding: 8px 12px;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: var(--parchment);
        font-size: 13.5px;
        transition: background 120ms;
    }
    .reading:last-child {
        margin-bottom: -4px;
        border-bottom: 0;
    }
    .row:hover, .row.active {
        background: var(--parchment-2);
    }
    .what {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .what b {
        font-weight: 700;
    }
    .what i {
        margin-left: 8px;
        color: var(--ink-faint);
        font-size: 12.5px;
        font-style: normal;
    }
    small {
        flex: none;
        max-width: 45%;
        overflow: hidden;
        color: var(--ink-faint);
        font-size: 12px;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .note {
        margin: 0;
        padding: 10px 12px;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .note + .note {
        padding-top: 0;
    }
    .note button {
        margin-left: 6px;
        padding: 2px 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 12.5px var(--sans);
    }
    .note button:hover {
        background: var(--parchment-2);
    }
    @media (prefers-reduced-motion: reduce) {
        .go {
            transition: background 120ms;
        }
    }
    .keys {
        display: none;
        flex-wrap: wrap;
        gap: 14px;
        margin: 0;
        padding: 7px 12px;
        border-top: 1px solid var(--line);
        color: var(--ink-faint);
        font-size: 12px;
    }
    @media (hover: hover) and (pointer: fine) {
        .keys {
            display: flex;
        }
        .reading kbd {
            display: inline;
        }
    }
    kbd {
        padding: 1px 5px;
        border: 1px solid var(--line-strong);
        border-radius: 4px;
        background: var(--parchment);
        color: var(--ink-soft);
        font: 11.5px var(--sans);
    }
    @container (max-width: 720px) {
        .field {
            flex: 1 1 100%;
            min-height: 46px;
            padding-right: 46px;
            border-bottom: 1px solid var(--line);
        }
        .list {
            width: auto;
        }
    }
</style>
