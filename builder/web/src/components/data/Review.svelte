<script lang="ts">
    import { onMount } from "svelte";
    import { edit, flagsOf, planArgs, type Flags } from "../../lib/data/flags";
    import { followers } from "../../lib/data/graph";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { leafSquare } from "../../lib/data/leaf";
    import { count, list, short, size, span, type Layer, type Review, type Router, type RunView, type Started, type Target } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import AreaPicker from "./AreaPicker.svelte";
    import Failure from "./Failure.svelte";

    // The update of target `name` in three steps: settings, a plan run, and the bake of that plan.
    // The plan is a run of `update --plan`: it fetches what the plan reads, with progress, and its
    // view gives the review. Start runs exactly the args of that review. `plan` is the run of the
    // URL; `locked` names the run that holds the lock of the target; `router` answers for R2.
    let {
        name,
        kind,
        flags: initial,
        plan: initialPlan = null,
        machine,
        locked,
        layers,
        area,
        router = null,
    }: {
        name: string;
        kind: Target["kind"];
        flags: Flags;
        plan?: string | null;
        machine: string;
        locked: string | null;
        layers: Layer[];
        area: Target["area"];
        router?: Router | null;
    } = $props();

    // svelte-ignore state_referenced_locally
    let flags = $state(initial);
    let edited = $state(false);
    // svelte-ignore state_referenced_locally
    let planned = $state<string | null>(initialPlan);
    const run = $derived(planned ? new CliQuery<RunView>(["runs", planned]) : null);
    let now = $state(Date.now());
    let stopPolling = () => {};
    $effect(() => {
        if (!run) return;
        void run.refresh();
        stopPolling = run.poll(2_000);
        return () => stopPolling();
    });
    onMount(() => {
        const tick = setInterval(() => (now = Date.now()), 1_000);
        return () => clearInterval(tick);
    });
    const view = $derived(run?.output ?? null);
    const live = $derived(view?.state === "running" || view?.state === "starting");
    $effect(() => {
        if (view && !live) stopPolling();
    });
    // A plan from the URL brings its settings, until the screen edits them.
    $effect(() => {
        if (view && !edited) flags = flagsOf(view.args);
    });
    const review = $derived(view?.state === "ok" ? (view.review as Review | null) : null);
    const outdated = $derived(!!view && JSON.stringify(flagsOf(view.args)) !== JSON.stringify(flags));
    const formats = $derived(review?.layers.filter((layer) => layer.format) ?? []);
    // The cells that bake, and those that the release drops.
    const leaves = (of: (layer: Review["layers"][number]) => string[]) => [...new Set(review?.layers.flatMap(of))];
    const [baking, dropped] = $derived([leaves((layer) => layer.leaves), leaves((layer) => layer.dropped_leaves)]);
    const cells = $derived(
        [...baking.map((leaf) => [leaf, "bakes"]), ...dropped.map((leaf) => [leaf, "goes"])].flatMap(([leaf, state]) => {
            const bounds = leafSquare(leaf);
            return bounds ? [{ bounds, color: state === "goes" ? "var(--missing)" : "var(--running)", label: `${leaf} · ${state}` }] : [];
        }),
    );
    let accepted = $state<string | null>(null);
    // `obc data update` asks before it changes an R2 target; the page asks with this screen.
    const start = $derived(review ? [...review.args, ...(kind === "r2" ? ["--yes"] : [])] : []);
    const blocked = $derived(
        !!locked || !review || outdated || !!review.machine.refusals.length || (formats.length > 0 && accepted !== review.id),
    );
    const placed = $derived(!!area || flags.some(([flag]) => flag === "--region" || flag === "--box"));

    const only = $derived(flags.find(([flag]) => flag === "--only")?.[1] ?? "");
    const scopes = $derived([
        ...new Set([...layers.filter((layer) => layer.state === "stale").map((layer) => layer.layer), ...(only ? [only] : [])]),
    ]);
    let refresh = $state("");
    /** Edit the settings; the same settings again change nothing. */
    function change(next: Flags) {
        if (JSON.stringify(next) === JSON.stringify(flags)) return;
        flags = next;
        edited = true;
    }
    const setArea = (flag: "--region" | "--box", value: string) => change(edit(flags, (old) => old === "--region" || old === "--box", [flag, value]));
    const setRefresh = () => {
        const source = refresh.trim().split("=")[0];
        change(edit(flags, (flag, value) => flag === "--refresh" && value.split("=")[0] === source, ["--refresh", refresh.trim()]));
        refresh = "";
    };
    const setLayer = (layer: string, on: boolean) =>
        change(edit(flags, (flag, value) => (flag === "--add" || flag === "--remove") && value === layer, [on ? "--add" : "--remove", layer]));
    const setScope = (layer: string) => change(edit(flags, (flag) => flag === "--only", layer ? ["--only", layer] : undefined));
    function planning(started: Started) {
        planned = started.run;
        history.replaceState(null, "", `#/${name}/review?plan=${encodeURIComponent(started.run)}`);
    }
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
    const area_of = (params: [string, string][]) => params.map(([, value]) => value).join(", ");
</script>


<section class="card" aria-labelledby="edits">
    <h2 id="edits">1 · Settings</h2>
    <p class="soft">Choose what {name} serves. Nothing runs until you plan.</p>
    {#if scopes.length}
        <label class="title">
            What to update
            <select value={only} onchange={(event) => setScope(event.currentTarget.value)}>
                <option value="">Everything stale ({layers.filter((layer) => layer.state === "stale").length} layers)</option>
                {#each scopes as layer (layer)}<option value={layer}>{scope(layer)}</option>{/each}
            </select>
        </label>
    {/if}
    <!-- A target without a region has no review: its first region comes from this picker. -->
    <fieldset class="area">
        <legend>Area · {name} serves {area?.name ?? "no area"}</legend>
        {#if cells.length}
            <p>
                <span class="running">■ {count(baking.length, "cell")} bake</span>{#if dropped.length}{" · "}<span class="missing"
                        >■ {count(dropped.length, "cell")} go</span
                    >{/if}
            </p>
        {/if}
        <AreaPicker region={review?.settings.region ?? ""} boxes={area?.boxes ?? []} {cells} apply={setArea} />
    </fieldset>
    {#if review}
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
            <button type="submit">Use this policy</button>
            <span class="soft">
                {#each Object.entries(review.settings.refresh) as [source, days], i (source)}{i ? " · " : ""}{source}: {days === "manual"
                        ? "manual"
                        : `${days} days`}{:else}Each source has the policy of data/sources.toml.{/each}
            </span>
        </form>
    {/if}
</section>

<section class="card" aria-labelledby="plan">
    <h2 id="plan">2 · Plan</h2>
    <p class="soft">
        The plan fetches what it needs to list the steps, such as the OSM extract of the area, and then shows what the update downloads, bakes and
        costs. It changes nothing on {name}. A first plan of an area can download several GB; the next plans reuse them.
    </p>
    {#if !placed}<p class="missing">Choose an area first.</p>{/if}
    <Action
        label={view ? "Plan again" : `Plan update on ${machine}`}
        args={planArgs(name, flags)}
        disabled={!placed || live || !!locked}
        ondone={planning}
    />
    {#if run?.error}<Failure error={run.error} />{/if}
    {#if view && (live || !review)}
        <div class="progress" class:running={live}>
            <div class="title">
                <b>{live ? "Planning" : `The plan ${view.state}`}</b>
                <span class="soft">{span(live ? now - Date.parse(view.started) : (view.wall_ms ?? 0))}</span>
                <span class="grow"></span>
                <a href="#/runs/{planned}">Run {planned}</a>
                {#if live}<Action label="Stop" args={["runs", planned!, "--stop"]} ondone={() => void run?.refresh()} />{/if}
            </div>
            {#if view.fetches.length}
                <ul>
                    {#each view.fetches as fetch, i (i)}
                        <li>
                            {fetch.source}{fetch.params.length ? ` (${area_of(fetch.params)})` : ""} · {fetch.error
                                ? `✗ ${fetch.error}`
                                : fetch.bytes === null
                                  ? "fetching…"
                                  : `${size(fetch.bytes)} in the store`}
                        </li>
                    {/each}
                </ul>
            {/if}
            {#if view.log.length}<pre>{view.log.slice(-8).join("\n")}</pre>{/if}
            {#if view.error}<Failure error={view.error} />{/if}
        </div>
    {/if}
</section>

{#if review && !live}
    {#if formats.length}
        <section class="card format" aria-labelledby="formats">
            <h2 id="formats">
                {formats.length === 1 ? "A format changes" : `${formats.length} formats change`}: readers need new code before they read the new data
            </h2>
            <div class="table">
                <table>
                    <thead><tr><th>Layer</th><th>Reader</th><th>Reads now</th><th>Needs</th><th>What happens</th></tr></thead>
                    <tbody>
                        {#each formats as { layer, format } (layer)}
                            {#each format?.readers ?? [] as { reader, effect } (reader)}
                                <tr><td>{layer}</td><td>{reader}</td><td>{format?.from}</td><td>{format?.to}</td><td class="effect">{effect}</td></tr>
                            {/each}
                        {/each}
                    </tbody>
                </table>
            </div>
        </section>
    {/if}

    <section class="card review" class:old={outdated} aria-labelledby="review">
        <h2 id="review">Plan {review.id}: update {name} on {machine}</h2>
        {#if outdated}<p class="missing">The settings changed since this plan. Plan again to start.</p>{/if}
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
        {#each review.notes as note (note.text)}<p class="missing"><b>{note.layer}</b> · {note.text}</p>{/each}
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
        {#if kind === "r2"}
            {@const reason = router?.serving?.reason}
            <p class={reason ? "missing" : ""}>
                <b>Router</b> · {router ? (reason ? `✗ ${reason}` : "serves the release of now") : "asking…"}. The run waits at Router ready, at
                most 24 hours, until the router serves the new release; {name} does not change before.
            </p>
            <p><b>Publishes plan {review.id} to {name}, which serves riders.</b></p>
        {/if}
        <Action
            label="Start bake on {machine}"
            args={start}
            disabled={blocked}
            ondone={(started: Started) => (location.hash = `#/runs/${started.run}`)}
        />
        <p class="soft">The run keeps going when you close this page or the SSH tunnel.</p>
    </section>
{/if}

<style>
    .review,
    .format {
        border: 2px solid var(--ink);
    }
    .old {
        opacity: 0.5;
    }
    .progress {
        display: grid;
        gap: 8px;
        padding: 12px 16px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
    }
    .progress.running {
        border-color: var(--running);
    }
    pre {
        margin: 0;
        overflow-x: auto;
        font-size: 12px;
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
    .format :is(th, td) {
        white-space: nowrap;
    }
    .format .effect {
        min-width: 16em;
        white-space: normal;
    }
</style>
