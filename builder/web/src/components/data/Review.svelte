<script lang="ts">
    import { command } from "../../lib/data/cli";
    import { edit, reviewArgs, type Flags } from "../../lib/data/flags";
    import { followers } from "../../lib/data/graph";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { list, short, size, span, type Layer, type Review, type Started, type Target } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import AreaPicker from "./AreaPicker.svelte";
    import Failure from "./Failure.svelte";

    // The review of an update of target `name`. An edit runs `--dry-run` again with its flags.
    // Start runs exactly the args of the review that the screen shows. `locked` names the run that
    // holds the lock of the target.
    let {
        name,
        kind,
        flags: initial,
        machine,
        locked,
        layers,
    }: { name: string; kind: Target["kind"]; flags: Flags; machine: string; locked: string | null; layers: Layer[] } = $props();

    // svelte-ignore state_referenced_locally
    let flags = $state(initial);
    // `name` is a string, so a status poll with the same target starts no new review.
    const query = $derived(new CliQuery<Review>(reviewArgs(name, flags)));
    $effect(() => void query.refresh());
    // The last review stays while the next one comes, but only a current review starts.
    let last = $state<Review | null>(null);
    $effect(() => {
        if (query.output) last = query.output;
    });
    const review = $derived(query.output ?? last);
    const formats = $derived(review?.layers.filter((layer) => layer.format) ?? []);
    let accepted = $state<string | null>(null);
    // `obc data update` asks before it changes an R2 target; the page asks with this screen.
    const start = $derived(review ? [...review.args, ...(kind === "r2" ? ["--yes"] : [])] : []);
    const blocked = $derived(
        !!locked || !query.output || !!query.output.machine.refusals.length || (formats.length > 0 && accepted !== query.output.id),
    );

    const only = $derived(flags.find(([flag]) => flag === "--only")?.[1] ?? "");
    const scopes = $derived([
        ...new Set([...layers.filter((layer) => layer.state === "stale").map((layer) => layer.layer), ...(only ? [only] : [])]),
    ]);
    let refresh = $state("");
    const setArea = (flag: "--region" | "--box", value: string) => (flags = edit(flags, (old) => old === "--region" || old === "--box", [flag, value]));
    const setRefresh = () => {
        const source = refresh.trim().split("=")[0];
        flags = edit(flags, (flag, value) => flag === "--refresh" && value.split("=")[0] === source, ["--refresh", refresh.trim()]);
        refresh = "";
    };
    const setLayer = (layer: string, on: boolean) =>
        (flags = edit(flags, (flag, value) => (flag === "--add" || flag === "--remove") && value === layer, [on ? "--add" : "--remove", layer]));
    const setScope = (layer: string) => (flags = edit(flags, (flag) => flag === "--only", layer ? ["--only", layer] : undefined));
    function scope(names: string): string {
        const after = names.split(",").flatMap((layer) => followers(layers, layer));
        return `Only ${names.replaceAll(",", ", ")}${after.length ? `, and ${list([...new Set(after)])} that follow` : ""}`;
    }
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
        {#if scopes.length}
            <label class="title">
                What to update
                <select value={only} onchange={(event) => setScope(event.currentTarget.value)}>
                    <option value="">Everything stale ({layers.filter((layer) => layer.state === "stale").length} layers)</option>
                    {#each scopes as layer (layer)}<option value={layer}>{scope(layer)}</option>{/each}
                </select>
            </label>
        {/if}
        <fieldset class="area">
            <legend>Area · now {review.settings.region}</legend>
            <AreaPicker region={review.settings.region} apply={setArea} />
        </fieldset>
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
    {#if !query.output}
        <p class={query.error ? "missing" : "soft"} role="status">
            {query.error ? "This review is out of date: the review of the new settings failed." : "Reviewing the new settings…"}
        </p>
    {/if}
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

    <section class="card review" class:old={!query.output} aria-labelledby="review">
        <h2 id="review">Review {review.id}: update {name} on {machine}</h2>
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
            <p><b>Download</b> · {review.downloads.files} files, {size(review.downloads.bytes)} from {name}</p>
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
                I understand that the readers of the old formats get no new data from {name} until they update.
            </label>
        {/if}
        {#if locked}<p class="missing">{name} is locked by {locked}. You can start when that run ends.</p>{/if}
        {#if kind === "r2"}<p><b>Publishes review {review.id} to {name}, which serves riders.</b></p>{/if}
        <Action
            label="Start on {machine}"
            args={start}
            disabled={blocked}
            ondone={(started: Started) => (location.hash = `#/runs/${started.run}`)}
        />
        <p class="soft">The run keeps going when you close this page or the SSH tunnel.</p>
    </section>
{:else if !query.error}
    <p class="soft">Reviewing the update of {name}…</p>
{/if}

<style>
    .review,
    .format {
        border: 2px solid var(--ink);
    }
    .old {
        opacity: 0.5;
    }
    select {
        min-height: 40px;
        max-width: 100%;
        font: inherit;
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
    .area legend {
        float: none;
        margin-bottom: 8px;
        font-weight: 600;
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
