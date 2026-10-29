<script lang="ts">
    import { counts, matches, sizeOf, STATUSES, statusNote, whereOf, type HeldStatus } from "../../lib/data/holding";
    import { list, type Status } from "../../lib/data/shapes";
    import { coverage, due, GROUPS, groupOf, type SourceRow } from "../../lib/data/sources";

    // Every source, by group, with how this machine holds it. A status of the summary filters the
    // list; what needs a hand comes first in its group.
    let { rows, status }: { rows: SourceRow[]; status: Status | null } = $props();

    let query = $state("");
    let shown = $state<HeldStatus[]>([]);
    const found = $derived(rows.filter((row) => matches(row, query)));
    const listed = $derived(found.filter((row) => !shown.length || shown.includes(row.held.status)));
    const groups = $derived([...new Set([...GROUPS, ...rows.map(groupOf)])]);
    const toggle = (status: HeldStatus) => (shown = shown.includes(status) ? shown.filter((at) => at !== status) : [...shown, status]);

    /** What a person acts on in a row: a login, a release due again, a lock of a stopped run. */
    function notes(row: SourceRow): { tone: string; text: string; href?: string }[] {
        const lock = coverage(row);
        return [
            ...(row.credential_missing ? [{ tone: "missing", text: `Needs a login: ${list(row.credential?.env ?? [row.credential?.file ?? ""])}. Set it on Machine.`, href: "#/machine" }] : []),
            ...due(row, status).map((text) => ({ tone: "stale", text })),
            ...(lock === "lock" ? [{ tone: "missing", text: "A run stopped without freeing the lock." }] : []),
            ...(lock === "preparing" ? [{ tone: "running", text: `Being prepared on ${row.prepared?.lock?.machine}.` }] : []),
        ];
    }
</script>

<div class="filters">
    <div class="summary" role="group" aria-label="Status">
        {#each counts(found) as [at, n] (at)}
            <button type="button" class="pill" data-status={at} aria-pressed={shown.includes(at)} onclick={() => toggle(at)}>{STATUSES[at]} <b>{n}</b></button>
        {/each}
    </div>
    <input type="search" aria-label="Search the sources" placeholder="Search: id, name, area or place" bind:value={query} />
</div>

{#each groups as group (group)}
    {@const inGroup = listed.filter((row) => groupOf(row) === group).sort((a, b) => notes(b).length - notes(a).length)}
    {#if inGroup.length}
        <details class="card" open>
            <summary><h2>{group}</h2> <span class="soft">{inGroup.length}</span></summary>
            <div class="table">
                <table>
                    <thead><tr><th>Source</th><th>Status</th><th>Covers</th><th>Where</th><th>Size</th><th>Date</th></tr></thead>
                    <tbody>
                        {#each inGroup as row (row.id)}
                            <tr>
                                <td class="what">
                                    <a href="#/sources/{encodeURIComponent(row.id)}">{row.use ?? row.id}</a>
                                    <code class="soft">{row.id}</code>
                                    {#each notes(row) as note (note.text)}
                                        {#if note.href}<a class={note.tone} href={note.href}>{note.text}</a>{:else}<span class={note.tone}>{note.text}</span>{/if}
                                    {/each}
                                </td>
                                <td>
                                    <span class="pill" data-status={row.held.status}>{STATUSES[row.held.status]}</span>
                                    {#each statusNote(row.held) as note (note)}<span class="soft note">{note}</span>{/each}
                                </td>
                                <td>{row.held.covers}</td>
                                <td class="place">{whereOf(row.held)}</td>
                                <td>{sizeOf(row.held)}</td>
                                <td>{row.held.date ?? "—"}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            </div>
        </details>
    {/if}
{:else}
    {#if rows.length}<p class="soft">No source matches.</p>{/if}
{/each}

<style>
    .filters {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 16px;
    }
    .summary {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
    }
    .summary .pill {
        min-height: 32px;
        padding: 0 12px;
        font-size: 13px;
    }
    .summary .pill:hover {
        background: color-mix(in srgb, var(--tone) 24%, var(--panel));
        border-color: var(--tone);
    }
    .summary .pill[aria-pressed="true"] {
        outline: 2px solid var(--tone);
        outline-offset: 1px;
    }
    input[type="search"] {
        flex: 1 1 220px;
        max-width: 360px;
    }
    summary {
        cursor: pointer;
    }
    summary h2 {
        display: inline;
    }
    table {
        min-width: 760px;
    }
    td {
        overflow-wrap: anywhere;
    }
    .what {
        width: 30%;
    }
    .what > * {
        display: block;
        margin-top: 2px;
    }
    .what > a:first-child {
        color: var(--ink);
        font-weight: 600;
    }
    .what :is(span, a.missing) {
        font-size: 13px;
        line-height: 18px;
    }
    .note {
        display: block;
        margin-top: 2px;
        font-size: 12px;
        line-height: 16px;
    }
    .place {
        font-size: 13px;
    }
    td:nth-child(5),
    td:nth-child(6) {
        white-space: nowrap;
    }
</style>
