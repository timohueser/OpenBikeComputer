<script lang="ts">
    import { tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import { savedWhen, versionSummary, versionTitle, type Version } from '../../lib/planner/versions';
    import type { Trip } from '../../lib/planner/editor';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { trip, line, versions, openId, draftSavedAt, draftError, onOpen, onSaveNew, onChange, onCompare }: {
        trip: Trip;
        /** The line of the trip, for the day count in a suggested name. */
        line?: RoutingLine;
        versions: Version[];
        /** The saved version that the trip belongs to. */
        openId?: string;
        draftSavedAt: number | null;
        draftError: string;
        onOpen: (id: string, name: string) => void;
        onSaveNew: (name: string) => Promise<void>;
        /** Renames or deletes versions. */
        onChange: (versions: Version[]) => Promise<void>;
        onCompare: () => void;
    } = $props();

    let open = $state(false);
    // `new` names the version about to be saved; `current` renames the open one.
    let naming = $state<'new' | 'current' | null>(null);
    let name = $state('');
    let error = $state('');
    let busy = $state(false);
    let root: HTMLDivElement;
    let menu = $state<HTMLDivElement>();
    let now = $state(Date.now());

    const current = $derived(versions.find(v => v.id === openId));
    const draft = $derived(draftError || (draftSavedAt === null ? 'Not saved yet' : `Saved in this browser · ${ago(draftSavedAt)}`));
    const suggested = $derived(`Version ${versions.length + (naming === 'new' ? 1 : 0)} · ${versionSummary(trip, line).split(' · ')[0]}`);

    function ago(time: number) {
        const minutes = Math.floor((now - time) / 60000);
        return minutes < 1 ? 'just now' : minutes < 60 ? `${minutes} min ago` : `at ${when(new Date(time).toISOString())}`;
    }

    const when = (iso: string) => savedWhen(iso, now);
    const title = (version: Version) => versionTitle(version, now);

    async function startNaming(what: 'new' | 'current') {
        now = Date.now();
        error = '';
        open = true;
        naming = what;
        name = what === 'current' ? current?.name ?? suggested : suggested;
        await tick();
        const input = menu?.querySelector<HTMLInputElement>('input');
        input?.focus();
        input?.select();
    }

    /** Saves or renames with the typed name; the suggested name left as it is saves an unnamed version. */
    async function finishNaming() {
        const what = naming;
        if (!what || busy) return;
        const typed = name.trim() === suggested ? '' : name;
        error = '';
        busy = true;
        try {
            if (what === 'new') await onSaveNew(typed);
            else await onChange(versions.map(v => v !== current ? v : { ...v, name: typed.trim() || undefined }));
        } catch {
            error = 'Could not save the version. Press Enter to try again.';
            return;
        } finally { busy = false; }
        naming = null;
        await tick();
        root.querySelector<HTMLElement>(what === 'new' ? '.save' : '.name-start')?.focus();
    }

    async function toggle() {
        open = !open;
        naming = null;
        if (!open) return;
        now = Date.now();
        error = '';
        await tick();
        menu?.querySelector<HTMLElement>('button')?.focus();
    }

    function choose(version: Version) {
        open = false;
        onOpen(version.id, title(version));
    }

    function compare() {
        open = false;
        onCompare();
    }

    async function remove(id: string) {
        const index = versions.findIndex(v => v.id === id);
        busy = true;
        try {
            await onChange(versions.filter(v => v.id !== id));
            error = '';
        } catch {
            error = 'Could not delete the version. Try again.';
        } finally { busy = false; }
        await tick();
        const rows = [...root.querySelectorAll<HTMLElement>('.delete')];
        (rows[Math.min(index, rows.length - 1)] ?? root.querySelector<HTMLElement>('.toggle'))?.focus();
    }

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open) return;
        event.stopPropagation();
        naming = null;
        open = false;
        root.querySelector<HTMLElement>('.toggle')?.focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="versions" bind:this={root} onkeydown={key}>
    <!-- Without a version, the button saves one; the list shows below the name. -->
    <button type="button" class="toggle" disabled={busy} aria-expanded={open} aria-haspopup="true"
        onclick={() => current || !trip.points.length ? toggle() : startNaming('new')}>
        {#if current}<span>Version</span> <strong>{title(current)}</strong>{:else}<strong>Save version</strong>{/if}
        <Icon name="down" size={14} />
    </button>
    {#if open}
        <div class="menu" bind:this={menu}>
            {#if naming}
                <form class="name" onsubmit={(event) => { event.preventDefault(); finishNaming(); }}>
                    <label>
                        <span>{naming === 'new' ? 'Save as' : 'Name this version'}</span>
                        <input aria-label="Version name" maxlength="60" bind:value={name} disabled={busy} />
                    </label>
                    <small>Enter saves · Esc cancels</small>
                    <button type="submit" class="planner-action" disabled={busy}>{busy ? 'Saving…' : 'Save checkpoint'}</button>
                </form>
            {:else}
                <p class="draft">{draft}</p>
            {/if}
            {#if error}<p class="error" role="alert">{error}</p>{/if}
            {#if versions.length}
                <ul>
                    {#each versions as version (version.id)}
                        <li class:current={version === current}>
                            <button type="button" class="what" disabled={busy} aria-current={version === current} onclick={() => version === current ? open = false : choose(version)}>
                                <strong>{title(version)}</strong>
                                <small>{version === current ? 'Open · ' : ''}{version.name ? `${when(version.at)} · ` : ''}{version.summary}</small>
                            </button>
                            <button type="button" class="delete" disabled={busy} aria-label={`Delete ${title(version)}`} onclick={() => remove(version.id)}><Icon name="close" size={14} /></button>
                        </li>
                    {/each}
                </ul>
            {:else if !naming}
                <p class="empty">No saved versions yet. Save a version to keep a copy you can return to.</p>
            {/if}
            {#if !naming}
                <button type="button" class="item save" disabled={busy || !trip.points.length} onclick={() => startNaming('new')}>Save as a new version…</button>
                {#if current}<button type="button" class="item name-start" disabled={busy} onclick={() => startNaming('current')}>Rename this version…</button>{/if}
            {/if}
            <button type="button" class="item" disabled={versions.length < 2} onclick={compare}>Compare versions</button>
        </div>
    {/if}
</div>

<style>
    .versions {
        position: relative;
    }
    .toggle {
        display: flex;
        align-items: center;
        gap: 8px;
        max-width: 260px;
        height: 32px;
        padding: 0 10px 0 14px;
        border: 1px solid var(--ink);
        border-radius: 6px;
        background: transparent;
        color: var(--ink);
        font: 600 13px var(--sans);
    }
    .toggle:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .toggle span {
        font-weight: 400;
        color: var(--ink-soft);
    }
    .toggle strong {
        overflow: hidden;
        text-overflow: ellipsis;
        font-weight: 600;
    }
    /* Offset from the right edge, so the open menu leaves the map controls column free. */
    .menu {
        position: absolute;
        top: 40px;
        right: 0;
        z-index: 30;
        width: min(340px, calc(100vw - 32px));
        padding: 8px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    .error { margin: 0; padding: 8px; color: var(--coral); font-size: 13px; }
    .draft,
    .empty {
        margin: 0;
        padding: 8px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    ul {
        margin: 0 0 4px;
        padding: 4px 0;
        list-style: none;
        border-block: 1px solid var(--line);
        max-height: 300px;
        overflow: auto;
    }
    li {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 4px 8px;
        border-radius: 6px;
    }
    li:hover,
    li.current {
        background: var(--parchment-2);
    }
    .what {
        flex: 1;
        min-width: 0;
        padding: 0;
        border: 0;
        background: none;
        text-align: left;
    }
    .what strong,
    .what small {
        display: block;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .what strong {
        font: 600 14px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .what small {
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
    button {
        font: inherit;
        color: inherit;
        cursor: pointer;
    }
    .delete {
        display: grid;
        place-items: center;
        width: 24px;
        height: 24px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-faint);
    }
    .delete:hover {
        color: var(--ink);
    }
    .item {
        width: 100%;
        padding: 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        text-align: left;
        font-size: 13px;
        font-weight: 600;
    }
    .item:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .item:disabled {
        color: var(--ink-faint);
        cursor: default;
    }
    .name {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 8px 8px 12px;
    }
    .name label {
        display: flex;
        flex-direction: column;
        gap: 4px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .name input {
        height: 32px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 13px var(--sans);
    }
    .name small {
        font-size: 11px;
        color: var(--ink-faint);
    }
</style>
