<script lang="ts">
    import { onMount } from "svelte";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { count, list, size, span, type Review, type RunView, type Started, type Target } from "../../lib/data/shapes";
    import type { SourceRow } from "../../lib/data/sources";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";
    import ReviewNote from "./ReviewNote.svelte";

    // The plan run: its downloads while it plans, then what the bake downloads, makes and costs
    // against this machine. Start runs exactly the args of the review.
    let {
        plan,
        target,
        machine,
        memory,
        free,
        sources,
        started,
    }: {
        plan: string;
        target: Target;
        machine: string;
        memory: number | null;
        free: number | null;
        sources: SourceRow[];
        started: (run: string) => void;
    } = $props();

    // svelte-ignore state_referenced_locally
    const run = new CliQuery<RunView>(["runs", plan]);
    let stop = () => {};
    let now = $state(Date.now());
    onMount(() => {
        stop = run.poll(2_000);
        const tick = setInterval(() => (now = Date.now()), 1_000);
        return () => {
            stop();
            clearInterval(tick);
        };
    });
    const view = $derived(run.output);
    const live = $derived(view?.state === "running" || view?.state === "starting");
    $effect(() => {
        if (view && !live) stop();
    });
    const review = $derived(view?.state === "ok" ? (view.review as Review | null) : null);
    const formats = $derived(review?.layers.filter((layer) => layer.format) ?? []);
    let accepted = $state(false);
    const blocked = $derived(!review || !!review.machine.refusals.length || (formats.length > 0 && !accepted) || !!(target.lock && !target.lock.died));
    const start = $derived(review ? [...review.args, ...(target.kind === "r2" ? ["--yes"] : [])] : []);

    const use = (id: string) => sources.find((source) => source.id === id)?.use ?? "";
    /** One row for each source version; a source fetched per tile or area counts its params. */
    const downloads = $derived.by(() => {
        const rows = new Map<string, { source: string; parts: number; bytes: number | null }>();
        for (const { source, version, params, bytes } of review?.fetches ?? []) {
            const key = `${source}@${version}`;
            const row = rows.get(key) ?? { source, parts: 0, bytes: 0 };
            row.parts += Math.max(params.length, 1);
            row.bytes = row.bytes === null || bytes === null ? null : row.bytes + bytes;
            rows.set(key, row);
        }
        return [...rows.values()];
    });
    const downloadBytes = $derived(downloads.reduce((total, row) => total + (row.bytes ?? 0), 0) + (review?.downloads.bytes ?? 0));
    const reused = $derived(new Map((review?.reused ?? []).map((build) => [build.step, build])));
    /** Whether the steps of `layer` build, or reuse the output of an earlier bake, with its date and
     *  for R2 its upload. */
    function how(layer: Review["layers"][number]): string {
        if (!layer.steps.length) return "drop";
        const old = layer.steps.flatMap((step) => reused.get(step) ?? []);
        if (!old.length) return "build";
        const built = old.map((build) => build.built ?? "").sort().at(-1)!.slice(0, 10);
        const upload = old.reduce((total, build) => total + (build.estimate?.client_bytes ?? 0), 0);
        const reuse = `(built ${built}${target.kind === "r2" ? `, ${size(upload)} to upload` : ""})`;
        return old.length === layer.steps.length ? `reuse ${reuse}` : `build ${layer.steps.length - old.length} · reuse ${old.length} ${reuse}`;
    }
    const PRODUCTS: Record<string, string> = { device: "Bike computer", planner: "Planner" };
    const makes = $derived(
        Object.entries(PRODUCTS).map(([product, name]) => ({
            name,
            layers: review?.layers.filter((layer) => layer.layer.startsWith(`${product}/`)) ?? [],
        })),
    );
    const unknown = $derived(review?.cost.unknown.estimate.length ?? 0);
    const share = (part: number | null, whole: number | null) => (part === null || !whole ? null : Math.min(part / whole, 1));
</script>

{#if run.error}<Failure error={run.error} />{/if}

{#if view && !review}
    <div class="planning">
        <p>
            <b class={live ? "running" : view.state === "ok" ? "" : "missing"}>{live ? "Making the plan" : `The plan ${view.state}`}</b>
            <span class="soft"> · {span(live ? now - Date.parse(view.started) : (view.wall_ms ?? 0))}</span>
        </p>
        {#if view.fetches.length}
            <ul class="fetches">
                {#each view.fetches as fetch, i (i)}
                    <li>
                        <span>{use(fetch.source) || fetch.source}</span>
                        <span class={fetch.error ? "missing" : fetch.bytes === null ? "running" : "soft"}
                            >{fetch.error ? fetch.error : fetch.bytes === null ? "downloading…" : size(fetch.bytes)}</span
                        >
                    </li>
                {/each}
            </ul>
        {/if}
        {#if view.error}<Failure error={view.error} />{/if}
        {#if live}<Action label="Stop planning" args={["runs", plan, "--stop"]} folded ondone={() => void run.refresh()} />{/if}
    </div>
{:else if !view && !run.error}
    <p class="soft">Reading the plan…</p>
{/if}

{#if review}
    <p class="lead">
        <b>{review.bakes.length ? `Builds ${count(review.bakes.length, "step")}` : "Builds nothing"}</b>{#if review.bakes.length}{" "}({unknown ===
                review.bakes.length
                    ? "time unknown"
                    : `${unknown ? "at least " : "about "}${span(review.cost.wall_ms)}`}){/if}
        · reuses {review.reused.length} from earlier bakes · downloads {downloads.length || review.downloads.files ? size(downloadBytes) : "nothing new"}{#if target.kind === "r2"}
            · uploads {size(review.cost.upload_bytes)}{/if}
        {#if unknown}<span class="soft">· {unknown} of {review.bakes.length} steps never ran here</span>{/if}
        {#if review.cost.unknown.guess.length}<span class="soft">· {review.cost.unknown.guess.length} guessed from other options</span>{/if}
    </p>
    <dl class="cost">
        <div>
            <dt>Peak memory</dt>
            <dd>
                {review.cost.peak_rss_bytes === null ? "unknown" : size(review.cost.peak_rss_bytes)}
                {#if memory}<span class="soft">of {size(memory)}</span>{/if}
                {#if share(review.cost.peak_rss_bytes, memory) !== null}
                    <meter min="0" max="1" low="0.6" high="0.85" optimum="0" value={share(review.cost.peak_rss_bytes, memory)}></meter>
                {/if}
            </dd>
        </div>
        <div>
            <dt>Disk</dt>
            <dd>
                {size(review.cost.store_bytes)}
                {#if free !== null}<span class="soft">of {size(free)} free</span>{/if}
                {#if share(review.cost.store_bytes, free) !== null}
                    <meter min="0" max="1" low="0.5" high="0.8" optimum="0" value={share(review.cost.store_bytes, free)}></meter>
                {/if}
            </dd>
        </div>
    </dl>

    {#each review.notes as note (note.text)}<ReviewNote {note} to={target.target} region={review.settings.region} tone="stale" />{/each}
    {#each review.services as problem (problem)}<p class="missing">✗ {problem} <a href="#/{target.target}">Open the services of {target.target}</a></p>{/each}
    {#if review.machine.refusals.length}<p class="missing">{machine} cannot bake this: {review.machine.refusals.join("; ")}</p>{/if}

    {#if downloads.length}
        <section aria-labelledby="downloads">
            <h3 id="downloads">Downloads during the bake</h3>
            <div class="table">
                <table>
                    <tbody>
                        {#each downloads as row (row.source)}
                            <tr>
                                <td><b>{row.source}</b>{#if use(row.source)}<span class="soft"> · {use(row.source)}</span>{/if}</td>
                                <td class="num">{row.parts > 1 ? `${row.parts} parts` : ""}</td>
                                <td class="num">{row.bytes === null ? "size unknown" : size(row.bytes)}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            </div>
        </section>
    {/if}

    <section aria-labelledby="makes">
        <h3 id="makes">Makes</h3>
        <div class="makes">
            {#each makes as product (product.name)}
                <div>
                    <b>{product.name}</b>
                    {#if product.layers.length}
                        <ul>
                            {#each product.layers as layer (layer.layer)}
                                <li>
                                    {layer.layer.split("/").slice(1).join("/")} · <b>{how(layer)}</b>{" · "}<span class="soft"
                                        >{layer.text}{layer.leaves.length ? ` · ${layer.leaves.length} tiles` : ""}</span
                                    >
                                </li>
                            {/each}
                        </ul>
                    {:else}
                        <p class="soft">nothing new</p>
                    {/if}
                </div>
            {/each}
        </div>
        <p class="soft">Keeps {review.kept} outputs of what {target.kind === "r2" ? "production" : "this computer"} serves now.</p>
    </section>

    {#if formats.length}
        <label class="gate">
            <input type="checkbox" bind:checked={accepted} />
            {list(formats.map((layer) => layer.layer))} change format: apps that read the old format get no new data until they update.
        </label>
    {/if}
    {#if target.lock && !target.lock.died}<p class="missing">Another bake runs now. Start when it ends.</p>{/if}

    <Action
        label={target.kind === "r2" ? `Bake on ${machine} and publish to production` : `Bake on ${machine}`}
        args={start}
        primary
        folded
        disabled={blocked}
        ondone={(output: Started) => started(output.run)}
    />
    <p class="soft">The bake keeps running when you close this page.</p>
{/if}

<style>
    .planning {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .fetches {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .fetches li {
        display: flex;
        justify-content: space-between;
        gap: 12px;
        padding: 4px 0;
        border-bottom: 1px solid var(--line);
    }
    .cost {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
        gap: 12px 24px;
        margin: 0;
    }
    .lead {
        margin: 0;
        font-size: 17px;
    }
    .cost dt {
        color: var(--ink-soft);
        font-size: 13px;
    }
    .cost dd {
        margin: 0;
        font-weight: 600;
        font-variant-numeric: tabular-nums;
    }
    .cost dd span {
        font-weight: 400;
    }
    meter {
        display: block;
        width: 100%;
        height: 8px;
        margin-top: 6px;
    }
    h3 {
        margin: 0 0 8px;
        font-size: 15px;
    }
    .num {
        text-align: right;
        white-space: nowrap;
        font-variant-numeric: tabular-nums;
    }
    .makes {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
        gap: 12px 24px;
    }
    .makes ul {
        margin: 4px 0 0;
        padding-left: 18px;
    }
    .gate {
        display: flex;
        gap: 12px;
        padding: 12px 16px;
        border: 1px solid var(--stale);
        border-radius: 8px;
        background: #fdf3e6;
    }
</style>
