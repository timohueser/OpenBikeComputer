<script lang="ts">
    import { tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import { newVersion, versionSummary, type Version } from '../../lib/planner/versions';
    import type { Trip } from '../../lib/planner/editor';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { trip, line, versions, draftSavedAt, draftError, onRestore, onChange }: {
        trip: Trip;
        /** The line of the trip, for the distance in a new version's summary. */
        line?: RoutingLine;
        versions: Version[];
        draftSavedAt: number | null;
        draftError: string;
        onRestore: (trip: Trip, name: string) => void;
        onChange: (versions: Version[]) => Promise<void>;
    } = $props();

    let open = $state(false);
    // `new` names the version about to be saved; `latest` renames the newest saved one.
    let naming = $state<'new' | 'latest' | null>(null);
    let name = $state('');
    let error = $state('');
    let busy = $state(false);
    let root: HTMLDivElement;
    let menu = $state<HTMLDivElement>();
    let now = $state(Date.now());

    const draft = $derived(draftError || (draftSavedAt === null ? 'Not saved yet' : `Saved in this browser · ${ago(draftSavedAt)}`));
    const suggested = $derived(`Version ${versions.length + (naming === 'new' ? 1 : 0)} · ${versionSummary(trip, line).split(' · ')[0]}`);

    function ago(time: number) {
        const minutes = Math.floor((now - time) / 60000);
        return minutes < 1 ? 'just now' : minutes < 60 ? `${minutes} min ago` : `at ${when(new Date(time).toISOString())}`;
    }

    function when(iso: string) {
        const at = new Date(iso);
        const days = Math.round((new Date(now).setHours(0, 0, 0, 0) - new Date(iso).setHours(0, 0, 0, 0)) / 86400000);
        const day = days === 0 ? 'Today' : days === 1 ? 'Yesterday' : at.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
        return `${day} ${at.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}`;
    }

    function title(version: Version) {
        return version.name ?? when(version.at);
    }

    async function startNaming(what: 'new' | 'latest') {
        now = Date.now();
        error = '';
        open = true;
        naming = what;
        name = what === 'latest' ? versions[0]?.name ?? suggested : suggested;
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
            if (what === 'new') await onChange([newVersion(trip, line, typed), ...versions]);
            else await onChange(versions.map((v, i) => i ? v : { ...v, name: typed.trim() || undefined }));
        } catch {
            error = 'Could not save the version. Press Enter to try again.';
            return;
        } finally { busy = false; }
        naming = null;
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

    function restore(version: Version) {
        open = false;
        onRestore(version.trip, title(version));
    }

    async function remove(id: string) {
        busy = true;
        try {
            await onChange(versions.filter(v => v.id !== id));
            error = '';
        } catch {
            error = 'Could not delete the version. Try again.';
        } finally { busy = false; }
    }

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open) return;
        event.stopPropagation();
        naming = null;
        open = false;
        root.querySelector<HTMLElement>('.more')?.focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="versions" bind:this={root} onkeydown={key}>
    <div class="split">
        <button type="button" class="save" disabled={busy || !trip.points.length} onclick={() => startNaming('new')}>Save version</button>
        <button type="button" class="more" disabled={busy} aria-label="Saved versions" aria-expanded={open} aria-haspopup="true" onclick={toggle}><Icon name="down" size={14} /></button>
    </div>
    {#if open}
        <div class="menu" bind:this={menu}>
            {#if naming}
                <form class="name" onsubmit={(event) => { event.preventDefault(); finishNaming(); }}>
                    <label>
                        <span>{naming === 'new' ? 'Save as' : 'Name the latest version'}</span>
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
                        <li>
                            <span class="what">
                                <strong>{title(version)}</strong>
                                <small>{version.name ? `${when(version.at)} · ` : ''}{version.summary}</small>
                            </span>
                            <button type="button" class="planner-action" disabled={busy} onclick={() => restore(version)}>Restore</button>
                            <button type="button" class="delete" disabled={busy} aria-label={`Delete ${title(version)}`} onclick={() => remove(version.id)}><Icon name="close" size={14} /></button>
                        </li>
                    {/each}
                </ul>
                {#if !naming}
                    <button type="button" class="name-start" onclick={() => startNaming('latest')}>Rename latest version…</button>
                {/if}
            {:else if !naming}
                <p class="empty">No saved versions yet. Save version keeps a copy you can return to.</p>
            {/if}
        </div>
    {/if}
</div>

<style>
    .versions {
        position: relative;
    }
    .split {
        display: flex;
        height: 32px;
        border: 1px solid var(--ink);
        border-radius: 6px;
        overflow: hidden;
    }
    .split button {
        border: 0;
        background: transparent;
        color: var(--ink);
        font: 600 13px var(--sans);
        cursor: pointer;
    }
    .split button:hover {
        background: var(--parchment-2);
    }
    .save {
        padding: 0 14px;
    }
    .more {
        display: grid;
        place-items: center;
        width: 30px;
    }
    .split button + button {
        border-left: 1px solid var(--ink);
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
    li:hover {
        background: var(--parchment-2);
    }
    .what {
        flex: 1;
        min-width: 0;
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
    .name-start {
        width: 100%;
        padding: 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        text-align: left;
        font-size: 13px;
        font-weight: 600;
    }
    .name-start:hover {
        background: var(--parchment-2);
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
