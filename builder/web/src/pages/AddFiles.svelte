<!-- The window "Add files": it reads GPX and FIT files, folders and ZIP exports, counts the rides and the planned routes
     in them, and adds them to the library. -->
<script lang="ts">
    import { onDestroy } from 'svelte';
    import { readFit } from '../lib/core/fit';
    import { readGpx } from '../lib/core/gpx';
    import { readFiles, type Found, type Item, type Readers } from '../lib/library/import';
    import type { Library } from '../lib/library/store';
    import { planFromFile } from '../lib/planner/gpx-import';
    import { formatDistance } from '../lib/settings/format';
    import Window from '../lib/ui/Window.svelte';

    let { library, onClose }: { library: Library; onClose: () => void } = $props();

    const readers: Readers = { gpx: readGpx, fit: readFit };
    const KINDS = [
        { kind: 'ride', label: 'rides' },
        { kind: 'route', label: 'planned routes' },
        { kind: 'known', label: 'are in your library already' },
        { kind: 'unreadable', label: 'not readable' },
    ] as const;

    // `found` is not reactive: a large export has thousands of records. `shown` follows `version`.
    const found: Found[] = [];
    /** The files that are added, so that "Add" after an error goes on with the others. */
    const done = new Set<Found>();
    let version = $state(0);
    let counts = $state({ ride: 0, route: 0, known: 0, unreadable: 0 });
    let source = $state('');
    let phase = $state<'empty' | 'reading' | 'read' | 'adding' | 'added'>('empty');
    let added = $state({ ride: 0, route: 0 });
    let message = $state('');
    let expanded = $state(false);
    let over = $state(false);
    let input: HTMLInputElement;
    let controller = new AbortController();

    const total = $derived(counts.ride + counts.route + counts.known + counts.unreadable);
    const toAdd = $derived(counts.ride + counts.route - added.ride - added.route);
    const shown = $derived((void version, expanded ? found.slice() : found.slice(0, 4)));
    const hidden = $derived((void version, found.length - shown.length));

    onDestroy(() => controller.abort());

    async function read(items: Item[]): Promise<void> {
        if (!items.length || phase === 'adding') return;
        controller.abort();
        const current = controller = new AbortController();
        found.length = 0;
        done.clear();
        counts ={ ride: 0, route: 0, known: 0, unreadable: 0 };
        added = { ride: 0, route: 0 };
        source = items.length === 1 ? items[0].name : '';
        message = '';
        expanded = false;
        phase = 'reading';
        version++;
        try {
            for await (const file of readFiles(items, library, readers, current.signal)) {
                // A file of a run that a new drop or Cancel stopped.
                if (current.signal.aborted) return;
                found.push(file);
                counts[file.kind]++;
                version++;
            }
        } catch (error) {
            if (current.signal.aborted) return;
            message = error instanceof Error ? error.message : String(error);
        }
        phase = 'read';
    }

    function drop(event: DragEvent): void {
        event.preventDefault();
        over = false;
        // The items of a drop can be read only during the event.
        const items = [...event.dataTransfer?.items ?? []].flatMap((item): Item[] => {
            if (item.kind !== 'file') return [];
            const entry = item.webkitGetAsEntry(), file = entry ? null : item.getAsFile();
            return entry ? [entry] : file ? [file] : [];
        });
        void read(items);
    }

    /** Writes one by one. A false from the library means another tab or an earlier file added the same record. */
    async function add(): Promise<void> {
        phase = 'adding';
        message = '';
        const { signal } = controller;
        for (let i = 0; i < found.length && !signal.aborted; i++) {
            const file = found[i];
            if (file.kind !== 'ride' && file.kind !== 'route' || done.has(file)) continue;
            let ok: boolean;
            try {
                ok = file.kind === 'ride' ? await library.addRide(file.ride, await file.open())
                    : await library.addRoute(await planFromFile(await file.open(), file.id, file.bike));
            } catch (error) {
                message = error instanceof Error ? error.message : String(error);
                phase = 'read';
                return;
            }
            done.add(file);
            if (ok) added[file.kind]++;
            else {
                counts[file.kind]--;
                counts.known++;
                found[i] = { file: file.file, name: file.name, kind: 'known' };
                version++;
            }
        }
        if (!signal.aborted) phase = 'added';
    }

    const plural = (n: number, one: string, many: string) => `${n.toLocaleString('en')} ${n === 1 ? one : many}`;

    function detail(file: Found): string {
        switch (file.kind) {
            case 'ride': return `ride · ${formatDistance(file.ride.summary.lengthM)}`;
            case 'route': return `planned route · ${formatDistance(file.lengthM)}`;
            case 'known': return 'in your library already';
            case 'unreadable': return `not readable: ${file.reason}`;
        }
    }
</script>

<Window title="Add files" {onClose}>
    {#snippet children(close)}
        <div
            class="drop"
            class:over
            role="region"
            aria-label="Drop files or a folder"
            ondragover={event => { event.preventDefault(); over = true; }}
            ondragleave={() => over = false}
            ondrop={drop}
        >
            <strong>Drop files or a folder here</strong>
            <p>GPX and FIT files, one or many. Also the ZIP file that Strava, Komoot or Garmin gives you with all your data.</p>
            <button type="button" onclick={() => input.click()} disabled={phase === 'adding'}>Choose files</button>
            <input bind:this={input} type="file" multiple hidden aria-label="Choose files"
                onchange={() => { void read([...input.files ?? []]); input.value = ''; }} />
        </div>

        {#if phase !== 'empty'}
            <section aria-label="Files read">
                <p class="head"><strong>{plural(total, 'file', 'files')} read</strong>{#if source}<span>from {source}</span>{/if}{#if phase === 'reading'}<span>reading…</span>{/if}</p>
                <div class="bar" aria-hidden="true">
                    {#each KINDS as { kind } (kind)}<span class={kind} style:flex-grow={counts[kind]}></span>{/each}
                </div>
                <dl class="counts">
                    {#each KINDS as { kind, label } (kind)}
                        <div><dt><i class={kind}></i>{label}</dt><dd>{counts[kind].toLocaleString('en')}</dd></div>
                    {/each}
                </dl>
                <p class="rule">A file with times is a ride. A file with no times is a planned route.</p>
                {#if shown.length}
                    <ul class:expanded>
                        {#each shown as file, i (i)}
                            <li><span class="name" title={file.file}>{file.name}</span><span class="detail" class:unreadable={file.kind === 'unreadable'}>{detail(file)}</span></li>
                        {/each}
                    </ul>
                    {#if hidden > 0}
                        <button type="button" class="more" onclick={() => expanded = true}>and {hidden.toLocaleString('en')} more ›</button>
                    {/if}
                {/if}
            </section>
        {/if}

        {#if message}<p class="message" role="alert">{message}</p>{/if}
        {#if phase === 'added'}
            <p class="message" role="status">Added {plural(added.ride, 'ride', 'rides')} and {plural(added.route, 'planned route', 'planned routes')}.</p>
        {/if}

        <footer>
            {#if phase === 'added'}
                <button type="button" class="primary" onclick={close}>Close</button>
            {:else}
                <button type="button" onclick={() => { controller.abort(); close(); }}>Cancel</button>
                <button type="button" class="primary" onclick={add} disabled={phase !== 'read' || toAdd === 0}>
                    {phase === 'adding' ? `Adding… ${toAdd.toLocaleString('en')} left` : `Add ${toAdd.toLocaleString('en')} to the library`}
                </button>
            {/if}
        </footer>
    {/snippet}
</Window>

<style>
    .drop {
        display: grid;
        justify-items: center;
        gap: 6px;
        padding: 20px 16px;
        border: 2px dashed var(--line-strong);
        border-radius: 10px;
        background: var(--parchment);
        text-align: center;
    }
    .drop.over { border-color: var(--forest); }
    .drop strong { font-size: 16px; }
    .drop p {
        margin: 0 0 6px;
        color: var(--ink-soft);
        font-size: 13px;
    }
    button {
        min-height: 32px;
        padding: 5px 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
    }
    button:hover:not(:disabled) { background: var(--parchment-2); }
    .primary {
        border-color: var(--forest);
        background: var(--forest);
        color: var(--panel);
    }
    .primary:hover:not(:disabled) { background: var(--forest-deep); }
    section { margin-top: 16px; }
    .head {
        display: flex;
        flex-wrap: wrap;
        align-items: baseline;
        gap: 4px 8px;
        margin: 0 0 8px;
    }
    .head span {
        color: var(--ink-soft);
        font-size: 12px;
    }
    .bar {
        display: flex;
        height: 8px;
        overflow: hidden;
        border-radius: 4px;
        background: var(--parchment-2);
    }
    .bar span { flex-basis: 0; }
    .ride { background: var(--forest); }
    .route { background: var(--route); }
    .known { background: var(--parchment-3); }
    .unreadable { background: var(--ink); }
    .counts {
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 8px;
        margin: 10px 0 0;
    }
    /* A term comes before its value in the markup; the number shows above its label. */
    .counts div {
        display: flex;
        flex-direction: column-reverse;
        justify-content: flex-end;
    }
    dd {
        margin: 0;
        font-size: 22px;
        font-weight: 700;
    }
    dt {
        color: var(--ink-soft);
        font-size: 12px;
    }
    dt i {
        display: inline-block;
        width: 9px;
        height: 9px;
        margin-right: 6px;
        border-radius: 2px;
    }
    .rule {
        margin: 12px 0 8px;
        color: var(--ink-soft);
        font-size: 12px;
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    ul.expanded {
        max-height: 40dvh;
        overflow: auto;
    }
    li {
        display: flex;
        justify-content: space-between;
        gap: 12px;
        padding: 7px 0;
        border-top: 1px solid var(--line);
        font-size: 13px;
    }
    .name {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .detail {
        flex: none;
        color: var(--ink-soft);
        font-size: 12px;
    }
    .detail.unreadable {
        background: none;
        color: var(--coral);
    }
    .more {
        min-height: 0;
        padding: 6px 0 0;
        border: 0;
        border-top: 1px solid var(--line);
        border-radius: 0;
        width: 100%;
        background: none;
        color: var(--ink-soft);
        font-size: 12px;
        text-align: left;
    }
    .more:hover:not(:disabled) { background: none; color: var(--ink); }
    .message { margin: 12px 0 0; }
    footer {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        margin: 16px -20px 0;
        padding: 14px 20px 0;
        border-top: 1px solid var(--line);
    }
    @media (max-width: 520px) {
        .counts { grid-template-columns: repeat(2, minmax(0, 1fr)); }
        .detail { max-width: 50%; text-align: right; }
    }
</style>
