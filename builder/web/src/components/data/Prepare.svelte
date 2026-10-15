<script lang="ts">
    import { onMount } from "svelte";
    import contentLanguages from "../../../../../specs/content-languages.json";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { list, size, span, when, type Started, type Target } from "../../lib/data/shapes";
    import { family, type PrepareReview, type SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";

    // With `rows`: the versions of a prepared source on each target, or the chips of a family, such as
    // `terrain-reference`. With `to`: the review of a new version for that target, and its Start,
    // which waits for `kind`, the kind of that target.
    let { id, rows = [], to, kind = null, region = "" }: { id: string; rows?: SourceRow[]; to?: string; kind?: Target["kind"] | null; region?: string } = $props();

    // svelte-ignore state_referenced_locally
    let area = $state(region);
    let worldwide = $state(false);
    let languages = $state(contentLanguages.map(([code]) => code));
    const languageName = new Intl.DisplayNames(["en"], { type: "language" });
    const args = $derived([
        "source", "prepare", id, "--to", to ?? "prod",
        ...(id === "wiki" ? [...(worldwide ? [] : ["--region", area.trim()]), "--languages", languages.join(",")] : []),
        "--dry-run",
    ]);
    let query = $state<CliQuery<PrepareReview> | null>(null);
    // A changed selection cannot start an earlier review, including a response still in flight.
    const review = $derived(query?.args.join("\0") === args.join("\0") ? query : null);
    function inspect() {
        query = new CliQuery<PrepareReview>(args);
        void query.refresh();
    }
    onMount(() => { if (to && id !== "wiki") inspect(); });
    const start = $derived(review?.output ? [...review.output.args, ...(kind === "r2" ? ["--yes"] : [])] : []);
    const names = $derived([...new Set(rows.flatMap((row) => Object.keys(row.prepared)))]);
    let target = $state("prod");
    function chip(row: SourceRow, name: string): string {
        const on = row.prepared[name];
        if (on?.error) return "cannot be read";
        if (on?.lock) return on.lock.stale ? `lock left on ${on.lock.machine}` : `preparing on ${on.lock.machine}`;
        return on?.versions.length ? `prepared ${on.versions.at(-1)!.version}` : "not prepared";
    }
</script>

{#if to}
    <h1>Prepare {id} for {to}</h1>
    <a href="#/sources/{encodeURIComponent(id)}">Back to {id}</a>
    {#if id === "wiki"}
        <form class="card options" onsubmit={(event) => { event.preventDefault(); inspect(); }}>
            <h2>Coverage and languages</h2>
            <label><input type="checkbox" bind:checked={worldwide} /> Worldwide</label>
            {#if !worldwide}
                <label>Geofabrik area <input required bind:value={area} placeholder="europe/dach" /></label>
            {/if}
            <fieldset>
                <legend>Article languages</legend>
                {#each contentLanguages as [code] (code)}
                    <label><input type="checkbox" bind:group={languages} value={code} /> {languageName.of(code)}</label>
                {/each}
            </fieldset>
            <p class="soft">Photos are fetched only for selected subjects in this coverage. The selected language archives are worldwide inputs.</p>
            <code>{command(args)}</code>
            <button type="submit" disabled={!languages.length || (!worldwide && !area.trim())}>Review preparation</button>
        </form>
    {/if}
    {#if review?.error}<Failure error={review.error} />{/if}
    {#if review?.output}
        {@const r = review.output}
        <section class="card review" aria-labelledby="prepare">
            <h2 id="prepare">Review {r.id}: version {r.version} of {r.source}</h2>
            {#if id === "wiki"}
                <p><b>Coverage</b> · {r.scope?.region ?? "Worldwide"} · <b>Languages</b> · {list(r.languages.map((code) => languageName.of(code) ?? code))}</p>
            {/if}
            <h3>Inputs</h3>
            <ul class="inputs">
                {#each r.inputs as input}
                    <li>
                        <span>{input.params.find(([key]) => key === "file")?.[1] ?? `${input.source} ${input.version ?? "newest"}`}</span>
                        <b>{input.bytes === null ? "Size unknown" : size(input.bytes)}{input.download_bytes === null ? "" : input.download_bytes === 0 ? " · Cached" : " · To download"}</b>
                    </li>
                {/each}
            </ul>
            {#if id === "wiki"}
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
                <b>Cost</b> · {r.cost.wall_ms === null ? "time unknown" : `about ${span(r.cost.wall_ms)} here`} · store {size(r.cost.store_bytes)}{r.cost
                    .upload_bytes === null
                    ? ""
                    : ` · upload ${size(r.cost.upload_bytes)}`}{r.cost.unknown.length ? ` · no size for ${list(r.cost.unknown)}` : ""}
            </p>
            <p><b>Peak memory</b> · {r.cost.peak_rss_bytes === null ? "unknown" : size(r.cost.peak_rss_bytes)} · <b>Upload</b> · {r.cost.upload_bytes === null ? "unknown" : size(r.cost.upload_bytes)}</p>
            <p>
                <b>This machine</b> · {#if r.machine.refusals.length}<span class="missing">cannot do it: {r.machine.refusals.join("; ")}</span
                    >{:else}no known refusal{/if}
            </p>
            {#if r.abandoned}<p><b>Abandoned</b> · the stopped version {r.abandoned} does not resume.</p>{/if}
            <p><b>When it finishes</b> · {list(r.stale.map((at) => `${at.target}: ${at.layers.length ? list(at.layers) : "nothing"} stale`))}</p>
            {#if kind === "r2"}<p><b>Publishes version {r.version} of {r.source} to {to}, which serves riders.</b></p>{/if}
            <Action
                label="Start"
                args={start}
                disabled={!kind || !!r.machine.refusals.length}
                ondone={(started: Started) => (location.hash = `#/runs/${started.run}`)}
            />
        </section>
    {:else if review && !review.error}
        <p class="soft">Reviewing the prepare…</p>
    {/if}
{:else}
    <section class="card" aria-labelledby="versions">
        <h2 id="versions">Prepared versions</h2>
        {#each names as name (name)}
            <div class="target">
                <b>{name}</b>
                {#if rows.length > 1 || rows[0]?.id !== id}
                    <div class="title">
                        {#each rows as row (row.id)}
                            <a class="chip" href="#/sources/{encodeURIComponent(row.id)}">{row.id.slice(family(row.id).length + 1)} · {chip(row, name)}</a>
                        {/each}
                    </div>
                {:else}
                    {@const on = rows[0].prepared[name]}
                    {@const serving = rows[0].targets[name]?.requests.map((request) => request.serving) ?? []}
                    {#if on.error}<span class="missing">{on.error}</span>{/if}
                    {#each on.versions as version (version.version)}
                        <span>{version.version} · {size(version.bytes)} · complete {when(version.time)}{serving.includes(version.version) ? " · serves" : ""}</span>
                    {:else}
                        <span class="soft">no version</span>
                    {/each}
                    {#if on.lock?.stale}
                        <span class="missing"
                            >No heartbeat from run {on.lock.run} on {on.lock.machine} since {when(on.lock.heartbeat)}: it likely ended without freeing the
                            lock. <code>{command(["unlock", name, "--source", id, "--from", on.lock.machine])}</code></span
                        >
                    {:else if on.lock}
                        <span class="running">Preparing on {on.lock.machine}: run {on.lock.run}, phase {on.lock.phase}</span>
                    {/if}
                {/if}
            </div>
        {/each}
        {#if rows.length === 1 && rows[0].id === id}
            <form class="title" onsubmit={(event) => (event.preventDefault(), (location.hash = `#/sources/${encodeURIComponent(id)}/prepare?to=${target}`))}>
                <label>Target <select bind:value={target}>{#each names as name (name)}<option value={name}>{name}</option>{/each}</select></label>
                <button type="submit">Prepare a new version…</button>
            </form>
        {/if}
    </section>
{/if}

<style>
    .options { display: flex; flex-direction: column; gap: 12px; }
    fieldset { display: flex; flex-wrap: wrap; gap: 16px; }
    .options code { overflow-wrap: anywhere; }
    .options input:not([type="checkbox"]) { margin-left: 8px; padding: 8px; font: inherit; }
    .options button { align-self: flex-start; }

    .inputs { padding-left: 20px; }
    .inputs li { margin: 8px 0; overflow-wrap: anywhere; }
    .inputs b { display: block; }

    .review {
        border: 2px solid var(--ink);
    }
    .target {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 8px 0;
        border-bottom: 1px solid var(--line);
    }
    .chip {
        padding: 2px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        text-decoration: none;
        color: inherit;
    }
</style>
