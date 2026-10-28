<script lang="ts">
    import contentLanguages from "../../../../../specs/content-languages.json";
    import { nameOf, type Areas } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { prepareHash, type BakeRoute } from "../../lib/data/screens";
    import { list, size, span, type Started } from "../../lib/data/shapes";
    import { modelsOf, referenceOf, type PrepareReview, type SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";
    import BakeArea from "./BakeArea.svelte";
    import Failure from "./Failure.svelte";

    // The review of a new version of reference `id` in the archive for an area, and its Start.
    let { id, rows, area, areas }: { id: string; rows: SourceRow[]; area: BakeRoute["area"]; areas: Areas | null } = $props();

    const wiki = $derived(id === "wiki");
    const reference = $derived(referenceOf(id));
    let whole = $state(false);
    let languages = $state(contentLanguages.map(([code]) => code));
    const languageName = new Intl.DisplayNames(["en"], { type: "language" });
    const covered = $derived(!whole && area);
    const args = $derived([
        "source", "prepare", id,
        ...(covered ? [area!.flag, area!.value] : []),
        ...(wiki ? ["--languages", languages.join(",")] : []),
        "--dry-run",
    ]);
    // The map outlines where a national model has data.
    const extent = $derived(
        modelsOf(id, rows).flatMap(({ row }) => (row.extent ? [{ west: row.extent[0], south: row.extent[1], east: row.extent[2], north: row.extent[3] }] : [])),
    );
    let query = $state<CliQuery<PrepareReview> | null>(null);
    // A changed selection cannot start an earlier review, including a response still in flight.
    const review = $derived(query?.args.join("\0") === args.join("\0") ? query : null);
    function inspect() {
        query = new CliQuery<PrepareReview>(args);
        void query.refresh();
    }
    const start = $derived(review?.output?.args ?? []);
</script>

<a href="#/references">All references</a>
<div class="title">
    <h1>Prepare {reference?.title ?? id}</h1>
    {#if reference}<span class="soft">{reference.about}</span>{/if}
</div>

<form class="card options" onsubmit={(event) => (event.preventDefault(), inspect())}>
        <h2>{wiki ? "Coverage and languages" : "Coverage"}</h2>
        {#if wiki}<label><input type="checkbox" bind:checked={whole} /> Worldwide</label>{/if}
        {#if !whole}
            <BakeArea {areas} chosen={area} served={extent} serving={null} choose={(next) => (location.hash = prepareHash(id, next))} />
            <p class:soft={!!area} class:missing={!area}>{area ? `Covers ${area.flag === "--box" ? `the box ${area.value}` : nameOf(area.value, areas)}` : "Choose an area."}</p>
        {/if}
        {#if wiki}
            <fieldset>
                <legend>Article languages</legend>
                {#each contentLanguages as [code] (code)}
                    <label><input type="checkbox" bind:group={languages} value={code} /> {languageName.of(code)}</label>
                {/each}
            </fieldset>
            <p class="soft">Photos come only for the subjects in this coverage. The language archives are worldwide downloads.</p>
        {/if}
        <button type="submit" class="primary" disabled={(wiki && !languages.length) || (!whole && !area)}>Review preparation</button>
        <details class="terminal"><summary>In a terminal</summary><code>{command(args)}</code></details>
    </form>

{#if review?.error}<Failure error={review.error} />{/if}
{#if review?.output}
    {@const r = review.output}
    <section class="card review" aria-labelledby="prepare">
        <h2 id="prepare">Version {r.version}</h2>
        <p>
            <b>Coverage</b> · {r.scope ? nameOf(r.scope.region, areas) : "Worldwide"}{#if wiki}
                · <b>Languages</b> · {list(r.languages.map((code) => languageName.of(code) ?? code))}{/if}
        </p>
        <h3>Inputs</h3>
        <ul class="inputs">
            {#each r.inputs as input}
                <li>
                    <span>{input.params.find(([key]) => key === "file")?.[1] ?? `${input.source} ${input.version ?? "newest"}`}</span>
                    <b>{input.bytes === null ? "Size unknown" : size(input.bytes)}{input.download_bytes === null ? "" : input.download_bytes === 0 ? " · Cached" : " · To download"}</b>
                </li>
            {/each}
        </ul>
        {#if wiki}
            <p><b>Download</b> · {size(r.inputs.reduce((total, input) => total + (input.download_bytes ?? 0), 0))}. Start downloads missing archives, then prepares the selected region. Interrupted downloads resume.</p>
        {/if}
        <p>
            <b>Files</b> · {r.files === null ? "counted by the run" : `up to ${r.files}`}{r.publish_text === null
                ? ""
                : r.publish_text
                  ? " · text published"
                  : " · text kept local"}
        </p>
        <p>
            <b>Cost</b> · {r.cost.wall_ms === null ? "time unknown" : `about ${span(r.cost.wall_ms)} here`} · store {size(r.cost.store_bytes)}{r.cost.unknown.length
                ? ` · no size for ${list(r.cost.unknown)}`
                : ""}
        </p>
        <p><b>Peak memory</b> · {r.cost.peak_rss_bytes === null ? "unknown" : size(r.cost.peak_rss_bytes)} · <b>Archive</b> · {r.cost.archive_bytes === null ? "unknown" : size(r.cost.archive_bytes)}</p>
        {#if r.machine.refusals.length}<p class="missing"><b>This machine cannot do it</b> · {r.machine.refusals.join("; ")}</p>{/if}
        {#if r.abandoned}<p><b>Abandoned</b> · the stopped version {r.abandoned} does not resume.</p>{/if}
        <p>
            <b>When it finishes</b> · {list(r.stale.map((at) => `${at.target}: ${at.layers.length ? list(at.layers) : "nothing"} to bake again`))}
        </p>
        <p><b>Keeps version {r.version} in the archive at {r.root}, where every bake here reads it.</b></p>
        <Action
            label="Start"
            args={start}
            primary
            folded
            disabled={!!r.machine.refusals.length}
            ondone={(started: Started) => (location.hash = `#/runs/${started.run}`)}
        />
    </section>
{:else if review && !review.error}
    <p class="soft">Reviewing the preparation…</p>
{/if}

<style>
    .options {
        gap: 12px;
    }
    fieldset {
        display: flex;
        flex-wrap: wrap;
        gap: 16px;
    }
    .options > button {
        align-self: flex-start;
    }
    .terminal summary {
        color: var(--ink-soft);
        cursor: pointer;
    }
    .terminal code {
        display: block;
        margin-top: 8px;
    }
    .inputs {
        padding-left: 20px;
    }
    .inputs li {
        margin: 8px 0;
        overflow-wrap: anywhere;
    }
    .inputs b {
        display: block;
    }
    .review {
        border: 1px solid var(--ink-faint);
    }
</style>
