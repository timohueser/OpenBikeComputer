<script lang="ts">
    import { command } from "../../lib/data/cli";
    import { edit, reviewArgs, type Flags } from "../../lib/data/flags";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { short, size, span, type Review, type Started, type Target } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";

    // The review of an update of `target`. An edit runs `--dry-run` again with its flags. Start runs
    // exactly the args of the review that the screen shows.
    let { target, flags: initial, machine }: { target: Target; flags: Flags; machine: string } = $props();

    // svelte-ignore state_referenced_locally
    let flags = $state(initial);
    const query = $derived(new CliQuery<Review>(reviewArgs(target.target, flags)));
    $effect(() => void query.refresh());
    // The last review stays while the next one comes, but only a current review starts.
    let last = $state<Review | null>(null);
    $effect(() => {
        if (query.output) last = query.output;
    });
    const review = $derived(query.output ?? last);
    const list = (items: string[]) => new Intl.ListFormat("en").format(items);
    const formats = $derived(review?.layers.filter((layer) => layer.format) ?? []);
    let accepted = $state<string | null>(null);
    // `obc data update` asks before it changes an R2 target; the page asks with this screen.
    const start = $derived(review ? [...review.args, ...(target.kind === "r2" ? ["--yes"] : [])] : []);
    const blocked = $derived(
        !query.output || !!query.output.machine.refusals.length || (formats.length > 0 && accepted !== query.output.id),
    );

    let region = $state("");
    let refresh = $state("");
    const setRegion = () => (flags = edit(flags, (flag) => flag === "--region" || flag === "--box", ["--region", region.trim()]));
    const setRefresh = () => {
        const source = refresh.trim().split("=")[0];
        flags = edit(flags, (flag, value) => flag === "--refresh" && value.split("=")[0] === source, ["--refresh", refresh.trim()]);
        refresh = "";
    };
    const setLayer = (name: string, on: boolean) =>
        (flags = edit(flags, (flag, value) => (flag === "--add" || flag === "--remove") && value === name, [on ? "--add" : "--remove", name]));
    function fetched(review: Review): string {
        if (!review.fetches.length) return "nothing: the store has each source";
        return list(review.fetches.map(({ source, version, bytes }) => `${source} ${version}${bytes === null ? "" : ` (${size(bytes)})`}`));
    }
    function cost({ cost }: Review): string {
        const parts = [`about ${span(cost.wall_ms)} here`, `store ${size(cost.store_bytes)}`, `upload ${size(cost.upload_bytes)}`];
        if (cost.peak_rss_bytes !== null) parts.splice(1, 0, `peak RAM ${size(cost.peak_rss_bytes)}`);
        if (cost.unknown.estimate.length) parts.push(`no estimate for ${list(cost.unknown.estimate)}`);
        return parts.join(" · ");
    }
</script>

<section class="card" aria-labelledby="edits">
    <h2 id="edits">Settings of this update</h2>
    <p class="soft">An edit makes a new review. The page keeps edits as flags: <code>{command(query.args)}</code></p>
    {#if review}
        <form class="title" onsubmit={(event) => (event.preventDefault(), setRegion())}>
            <label>Area <input bind:value={region} placeholder={review.settings.region} required /></label>
            <button type="submit">Review this area</button>
            <span class="soft">A preset of data/regions/, or Geofabrik paths joined by a comma.</span>
        </form>
        {#if review.optional.length}
            <fieldset class="title">
                <legend>Optional layers</legend>
                {#each review.optional as name (name)}
                    <label>
                        <input
                            type="checkbox"
                            checked={review.settings.layers.includes(name)}
                            onchange={(event) => setLayer(name, event.currentTarget.checked)}
                        />
                        {name}
                    </label>
                {/each}
            </fieldset>
        {/if}
        <form class="title" onsubmit={(event) => (event.preventDefault(), setRefresh())}>
            <label>Refresh <input bind:value={refresh} placeholder="wiki=30 or wiki=manual" required /></label>
            <button type="submit">Review this policy</button>
            <span class="soft">
                {#each Object.entries(review.settings.refresh) as [source, days], i (source)}{i ? " · " : ""}{source}: {days === "manual"
                        ? "manual"
                        : `${days} days`}{:else}Each source has the policy of data/sources.toml.{/each}
            </span>
        </form>
    {/if}
</section>

{#if query.error}<Failure error={query.error} />{/if}

{#if review}
    {#if formats.length}
        <section class="card format" aria-labelledby="formats">
            <h2 id="formats">
                {formats.length === 1 ? "A format changes" : `${formats.length} formats change`}: readers need new code before they read the new data
            </h2>
            {#each formats as layer (layer.layer)}
                <p><b>{layer.layer}</b> {layer.format?.from} → {layer.format?.to} · read by {list(layer.readers)}</p>
            {/each}
        </section>
    {/if}

    <section class="card review" aria-labelledby="review">
        <h2 id="review">Review {review.id}: update {target.target} on {machine}</h2>
        <p><b>Fetch</b> · {fetched(review)}</p>
        {#if review.sources.length}
            <p><b>Sources</b> · {list(review.sources.map(({ source, from, to }) => `${source} ${from ?? "new"} → ${to}`))}</p>
        {/if}
        {#if review.layers.length}
            <div>
                <b>Bake</b>
                <ul>
                    {#each review.layers as layer (layer.layer)}
                        <li>
                            {layer.layer}{layer.leaves.length ? ` (${layer.leaves.length} cells)` : ""} · {layer.text}{layer.drops.length
                                ? ` · drops ${layer.drops.length} steps`
                                : ""} · read by {list(layer.readers)}
                        </li>
                    {/each}
                </ul>
            </div>
        {:else}
            <p><b>Bake</b> · nothing: each layer is as its steps make it now</p>
        {/if}
        <p><b>Keep</b> · {review.kept} outputs of the serving release</p>
        {#if review.downloads.files}
            <p><b>Download</b> · {review.downloads.files} files, {size(review.downloads.bytes)} from {target.target}</p>
        {/if}
        <p><b>Cost</b> · {cost(review)}</p>
        <p>
            <b>This machine</b> ·
            {#if review.machine.refusals.length}<span class="missing">cannot do it: {review.machine.refusals.join("; ")}</span>{:else}can do it{/if}
        </p>
        <p>
            <b>After</b> · {review.after.rollback ? `${short(review.after.rollback)} becomes the rollback` : "nothing becomes the rollback"}{review
                .after.collects
                ? `; ${short(review.after.collects.release)} is deleted (${review.after.collects.keys} keys, ${size(review.after.collects.bytes)})`
                : ""}.
        </p>
        {#if formats.length}
            <label class="gate">
                <input
                    type="checkbox"
                    checked={accepted === review.id}
                    onchange={(event) => (accepted = event.currentTarget.checked ? review.id : null)}
                />
                I understand that the readers of the old formats get no new data from {target.target} until they update.
            </label>
        {/if}
        <Action
            label="Start on {machine}"
            args={start}
            disabled={blocked}
            ondone={(started: Started) => (location.hash = `#/runs/${started.run}`)}
        />
        <p class="soft">The run keeps going when you close this page or the SSH tunnel.</p>
    </section>
{:else if !query.error}
    <p class="soft">Reviewing the update of {target.target}…</p>
{/if}

<style>
    .review,
    .format {
        border: 2px solid var(--ink);
    }
    .format,
    .gate {
        border-color: var(--stale);
        background: #fdf3e6;
    }
    .gate {
        display: flex;
        gap: 12px;
        padding: 12px 16px;
        border: 1px solid var(--stale);
        border-radius: 8px;
    }
    fieldset {
        margin: 0;
        padding: 0;
        border: 0;
    }
    legend {
        float: left;
        margin-right: 8px;
    }
    input:not([type="checkbox"]) {
        min-height: 40px;
        margin-left: 8px;
        padding: 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        font: inherit;
    }
    ul {
        margin: 4px 0 0;
        padding-left: 22px;
    }
</style>
