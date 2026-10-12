<script lang="ts">
    import { onMount } from "svelte";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { list, size, span, when, type Started, type Target } from "../../lib/data/shapes";
    import { family, type PrepareReview, type SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";

    // With `rows`: the versions of a prepared source on each target, or the chips of a family, such as
    // `terrain-reference`. With `to`: the review of a new version for that target, and its Start,
    // which waits for `kind`, the kind of that target.
    let { id, rows = [], to, kind = null }: { id: string; rows?: SourceRow[]; to?: string; kind?: Target["kind"] | null } = $props();

    // svelte-ignore state_referenced_locally
    const review = to ? new CliQuery<PrepareReview>(["source", "prepare", id, "--to", to, "--dry-run"]) : null;
    onMount(() => void review?.refresh());
    // A prepare asks before it changes an R2 target; the page asks with this screen.
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

{#if review}
    <h1>Prepare {id} for {to}</h1>
    <a href="#/sources/{encodeURIComponent(id)}">Back to {id}</a>
    {#if review.error}<Failure error={review.error} />{/if}
    {#if review.output}
        {@const r = review.output}
        <section class="card review" aria-labelledby="prepare">
            <h2 id="prepare">Review {r.id}: version {r.version} of {r.source}</h2>
            <p><b>Inputs</b> · {list(r.inputs.map((input) => `${input.source} ${input.version ?? "newest"}${input.bytes === null ? "" : ` (${size(input.bytes)})`}`))}</p>
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
            <p>
                <b>This machine</b> · {#if r.machine.refusals.length}<span class="missing">cannot do it: {r.machine.refusals.join("; ")}</span
                    >{:else}can do it{/if}
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
    {:else if !review.error}
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
