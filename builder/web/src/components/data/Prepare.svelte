<script lang="ts">
    import { onMount } from "svelte";
    import contentLanguages from "../../../../../specs/content-languages.json";
    import { boundsOf, nameOf, parseBox, type Areas } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { prepareHash, type BakeRoute } from "../../lib/data/screens";
    import { list, size, span, type Started, type Target } from "../../lib/data/shapes";
    import { family, FAMILIES, type PrepareReview, type SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";
    import BakeArea from "./BakeArea.svelte";
    import Failure from "./Failure.svelte";

    // The review of a new version of reference `id` on target `to`, and its Start. Wiki and a version
    // for this computer cover an area; R2 holds all of a terrain or forest member.
    let {
        id,
        row,
        to,
        area,
        targets,
        areas,
    }: { id: string; row: SourceRow | null; to: string; area: BakeRoute["area"]; targets: Target[]; areas: Areas | null } = $props();

    const kind = $derived(targets.find((at) => at.target === to)?.kind ?? null);
    const wiki = $derived(id === "wiki");
    const title = $derived(FAMILIES.find((at) => at.family === family(id))?.title ?? id);
    let whole = $state(false);
    let languages = $state(contentLanguages.map(([code]) => code));
    const languageName = new Intl.DisplayNames(["en"], { type: "language" });
    // Only `wiki` publishes a region to `prod`; the other producers prepare all of it there.
    const scoped = $derived(wiki || to !== "prod");
    const covered = $derived(scoped && !whole && area);
    const args = $derived([
        "source", "prepare", id, "--to", to,
        ...(covered ? [area!.flag, area!.value] : []),
        ...(wiki ? ["--languages", languages.join(",")] : []),
        "--dry-run",
    ]);
    const bounds = $derived(area ? (area.flag === "--box" ? parseBox(area.value) : areas && boundsOf(area.value, areas)) : null);
    const extent = $derived(row?.extent ? [{ west: row.extent[0], south: row.extent[1], east: row.extent[2], north: row.extent[3] }] : []);
    let query = $state<CliQuery<PrepareReview> | null>(null);
    // A changed selection cannot start an earlier review, including a response still in flight.
    const review = $derived(query?.args.join("\0") === args.join("\0") ? query : null);
    function inspect() {
        query = new CliQuery<PrepareReview>(args);
        void query.refresh();
    }
    onMount(() => {
        if (!scoped) inspect();
    });
    const start = $derived(review?.output ? [...review.output.args, ...(kind === "r2" ? ["--yes"] : [])] : []);
    const where = (target: string) => (targets.find((at) => at.target === target)?.kind ?? (target === "prod" ? "r2" : "dir")) === "r2";
</script>

<a href="#/references">All references</a>
<div class="title">
    <h1>Prepare {title}</h1>
    {#if !wiki && row?.use}<span class="soft">{row.use}</span>{/if}
</div>

{#if targets.length}
    <section class="card options" aria-labelledby="where">
        <h2 id="where">Where it goes</h2>
        <div class="where" role="radiogroup" aria-labelledby="where">
            {#each targets as at (at.target)}
                <button type="button" role="radio" aria-checked={at.target === to} onclick={() => (location.hash = prepareHash(id, at.target, area))}>
                    <b>{at.kind === "r2" ? "On R2" : "On this computer only"}</b>
                    <span>{at.kind === "r2" ? "Every bake can use it" : "Only the bakes here"}</span>
                </button>
            {/each}
        </div>
    </section>
{/if}

{#if scoped}
    <form class="card options" onsubmit={(event) => (event.preventDefault(), inspect())}>
        <h2>{wiki ? "Coverage and languages" : "Coverage"}</h2>
        <label><input type="checkbox" bind:checked={whole} /> {wiki ? "Worldwide" : "All of it"}</label>
        {#if !whole}
            <BakeArea {areas} chosen={area} {bounds} served={extent} choose={(next) => (location.hash = prepareHash(id, to, next))} />
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
{/if}

{#if review?.error}<Failure error={review.error} />{/if}
{#if review?.output}
    {@const r = review.output}
    <section class="card review" aria-labelledby="prepare">
        <h2 id="prepare">Version {r.version}</h2>
        <p>
            <b>Coverage</b> · {r.scope ? nameOf(r.scope.region, areas) : wiki ? "Worldwide" : "All of it"}{#if wiki}
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
        <p><b>Peak memory</b> · {r.cost.peak_rss_bytes === null ? "unknown" : size(r.cost.peak_rss_bytes)} · <b>Upload</b> · {r.cost.upload_bytes === null ? "none" : size(r.cost.upload_bytes)}</p>
        {#if r.machine.refusals.length}<p class="missing"><b>This machine cannot do it</b> · {r.machine.refusals.join("; ")}</p>{/if}
        {#if r.abandoned}<p><b>Abandoned</b> · the stopped version {r.abandoned} does not resume.</p>{/if}
        <p>
            <b>When it finishes</b> · {list(r.stale.map((at) => `${where(at.target) ? "Production" : "This computer"}: ${at.layers.length ? list(at.layers) : "nothing"} to bake again`))}
        </p>
        {#if kind === "r2"}<p><b>Publishes version {r.version} to R2, where every bake can read it.</b></p>{/if}
        <Action
            label="Start"
            args={start}
            primary
            folded
            disabled={!kind || !!r.machine.refusals.length}
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
    .where {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }
    .where button {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        min-height: 56px;
        padding: 8px 16px;
        border-color: var(--line-strong);
        font-weight: 400;
        text-align: left;
    }
    .where button span {
        color: var(--ink-soft);
        font-size: 13px;
        line-height: 18px;
    }
    .where button[aria-checked="true"] {
        border-color: var(--ink);
        box-shadow: inset 0 0 0 1px var(--ink);
    }
    .where button:focus-visible {
        outline: 2px solid var(--running);
        outline-offset: 2px;
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
        border: 2px solid var(--ink);
    }
</style>
