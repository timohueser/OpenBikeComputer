<script lang="ts">
    import { command } from "../../lib/data/cli";
    import { flagsOf, reviewArgs, type Flags } from "../../lib/data/flags";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { targetAt } from "../../lib/data/screens";
    import { causes, clock, failure, LAYER_MARK, layerText, short, version, when, type RunView, type Status } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";
    import Review from "./Review.svelte";

    // A target: what it serves, its lock, its layers and its releases; or the review of its update.
    let { name, hash, status }: { name: string; hash: string; status: CliQuery<Status> } = $props();

    const target = $derived(status.output?.targets.find((at) => at.target === name) ?? null);
    const machine = $derived(status.output?.machine.name ?? "this machine");
    const route = $derived(targetAt(hash));
    // The review that resumes a run starts from the settings flags of that run.
    const resumed = $derived(route.resume ? new CliQuery<RunView>(["runs", route.resume]) : null);
    $effect(() => void resumed?.refresh());

    const lock = $derived(target?.lock ?? null);
    // A live run holds the lock: an update or a rollback is refused until it ends.
    const locked = $derived(lock && !lock.stale ? `run ${lock.run} on ${lock.machine}` : null);
    const stale = $derived(target?.layers?.filter((layer) => layer.state === "stale") ?? []);
    // The page asks before it runs a command that changes an R2 target, so it sends `--yes`.
    const yes = $derived(target?.kind === "r2" ? ["--yes"] : []);
    // A ticked layer bakes again, also when it is up to date: `--only`.
    let ticked = $state<string[]>([]);
    const only = $derived(route.only ? ([["--only", route.only]] as Flags) : []);

    let asking = $state<"rollback" | "forward" | "unlock" | null>(null);
    // The banner after a change stays while the target is as before the change or as after it.
    const now = $derived(target ? `${target.release?.serving.id ?? ""}|${lock?.run ?? ""}` : "");
    let done = $state<{ text: string; before: string; after: string; forward: string | null } | null>(null);
    // A confirm stays while its command runs: the run of a rollback holds the lock.
    $effect(() => {
        if (done && !asking && now !== done.before && now !== done.after) done = null;
    });
    function changed(text: string, after: string, forward: string | null = null) {
        done = { text, before: now, after, forward };
        asking = null;
        void status.refresh();
    }
</script>

{#snippet confirm(title: string, lines: string[], label: string, args: string[], ondone: () => void)}
    <div class="confirm">
        <b>{title}</b>
        {#each lines as line (line)}<p>{line}</p>{/each}
        <div class="title">
            <Action {label} {args} {ondone} />
            <button type="button" onclick={() => (asking = null)}>Cancel</button>
        </div>
    </div>
{/snippet}

<div class="title">
    <h1>{name}</h1>
    {#if target}
        <span class="soft">
            {target.kind === "r2" ? "R2 bucket" : "folder"}
            {target.at} · {target.release ? `release ${short(target.release.serving.id)} since ${when(target.release.serving.since)}` : "serves nothing"}
        </span>
    {/if}
</div>

{#if !target}
    <p class="soft">{status.output ? `data/targets.toml has no target ${name}.` : `Reading ${name}…`}</p>
{:else if route.review}
    <a href="#/{name}">Back to {name}</a>
    {#if resumed?.error}<Failure error={resumed.error} />{/if}
    {#if !route.resume}
        {#key route.only}
            <Review {name} kind={target.kind} flags={only} {machine} {locked} layers={target.layers ?? []} />
        {/key}
    {:else if resumed?.output}
        <p class="soft">This review resumes run <a href="#/runs/{route.resume}">{route.resume}</a> with its settings.</p>
        <Review {name} kind={target.kind} flags={flagsOf(resumed.output.args)} {machine} {locked} layers={target.layers ?? []} />
    {/if}
{:else}
    {@const error = failure(target)}
    {#if error}<Failure {error} />{/if}
    {#if target.lock_error}<p class="missing">The lock of {name} cannot be read: {target.lock_error}</p>{/if}
    {#if done}
        {@const forward = done.forward}
        <section class="card done" role="status">
            <p><b>{done.text}</b></p>
            {#if forward && asking === "forward"}
                {@render confirm(
                    `Go forward to ${short(forward)}?`,
                    [`${name} serves ${short(forward)} again. Nothing is deleted.`],
                    "Go forward now",
                    ["rollback", name, "--expect", forward, ...yes],
                    () => changed(`${name} serves ${short(forward)} again.`, `${forward}|`),
                )}
            {:else if forward}
                <button type="button" onclick={() => (asking = "forward")}>Go forward to {short(forward)}…</button>
            {/if}
        </section>
    {/if}

    {#if lock}
        <section class="card lock" aria-labelledby="lock">
            <h2 id="lock">{lock.machine} is updating {name}</h2>
            <p>Run {lock.run} · phase {lock.phase}{lock.waiting ? `: ${lock.waiting}` : ""} · last heartbeat {clock(lock.heartbeat)}</p>
            {#if !lock.stale}
                <p>One machine updates a target at a time. You can update or roll back {name} when this run ends, or watch it now.</p>
                <a class="primary" href="#/runs/{lock.run}">Watch the run</a>
            {:else if asking === "unlock"}
                {@render confirm(
                    `Take over the ${name} lock from ${lock.machine}?`,
                    [
                        `If that run still runs, it sees that it lost the lock before it switches ${name}, and stops. ${name} never gets two updates at once.`,
                        `Its finished layers stay on ${lock.machine}.`,
                    ],
                    "Take over the lock",
                    ["unlock", name, "--from", lock.machine, "--yes"],
                    () => changed(`The ${name} lock is free.`, `${target.release?.serving.id ?? ""}|`),
                )}
            {:else}
                <p class="missing">No heartbeat since {when(lock.heartbeat)}: the run likely ended without freeing the lock. Open {lock.machine} first.</p>
                <button type="button" onclick={() => (asking = "unlock")}>Take over the lock…</button>
            {/if}
        </section>
    {/if}

    <div class="grid">
        <section class="card" aria-labelledby="layers">
            <h2 id="layers">Layers{target.region ? ` of ${target.region}` : ""}</h2>
            <p class="soft">Tick a layer to bake it again, also when it is up to date.</p>
            {#each target.layers ?? [] as layer (layer.layer)}
                <label class="layer">
                    <input type="checkbox" bind:group={ticked} value={layer.layer} disabled={layer.state === "off"} />
                    <span class={layer.state} aria-hidden="true">{LAYER_MARK[layer.state]}</span>
                    <span class="name"><b>{layer.layer}</b><span class={layer.state === "ok" ? "soft" : layer.state}>{layerText(layer)}</span></span>
                    {#if layer.format !== null}<code>{version(layer.format, layer.revision)}</code>{/if}
                </label>
            {:else}
                <p class="soft">{target.release ? "The steps of this target cannot be listed." : `${name} serves nothing yet.`}</p>
            {/each}
            {#each target.blocked as blocked (blocked.step)}
                <p class="missing">{blocked.layer} cannot be planned: {blocked.reason}</p>
            {/each}
        </section>

        <div class="column">
            <section class="card" aria-labelledby="update">
                <h2 id="update">Update {name}</h2>
                {#if stale.length}
                    <p><b class="stale">{stale.length} layers stale.</b> Causes: {causes(stale)}.</p>
                {:else if target.layers}
                    <p>Each layer is up to date.</p>
                {/if}
                {#if target.drift.length}<p class="missing">These keys differ from the serving release: {target.drift.join(", ")}</p>{/if}
                {#if locked}
                    <span class="primary locked" aria-disabled="true">Review update: locked</span>
                {:else}
                    <a class="primary" href="#/{name}/review{ticked.length ? `?only=${encodeURIComponent(ticked.join(','))}` : ''}">Review update</a>
                    {#if ticked.length}<code>{command(reviewArgs(name, [["--only", ticked.join(",")]]))}</code>{/if}
                {/if}
            </section>

            {#if target.release}
                {@const { serving, rollback } = target.release}
                <section class="card" aria-labelledby="releases">
                    <h2 id="releases">Releases on {name}</h2>
                    <div class="release"><b>{short(serving.id)}</b><span>since {when(serving.since)}</span><span class="pill">serving</span></div>
                    {#if rollback}
                        <div class="release">
                            <b>{short(rollback.id)}</b><span>since {when(rollback.since)}</span>
                            {#if asking !== "rollback"}
                                <button type="button" disabled={!!locked} onclick={() => (asking = "rollback")}>Roll back to {short(rollback.id)}…</button>
                            {/if}
                        </div>
                        {#if asking === "rollback"}
                            {@render confirm(
                                `Roll back ${name} to ${short(rollback.id)}?`,
                                [
                                    `${name} serves ${short(rollback.id)} instead of ${short(serving.id)}.`,
                                    `The router still runs ${short(rollback.id)}, so the switch is instant. Clients switch within a minute.`,
                                    `Nothing is deleted: ${short(serving.id)} becomes the rollback.`,
                                ],
                                "Roll back now",
                                ["rollback", name, "--expect", rollback.id, ...yes],
                                () => changed(`${name} serves ${short(rollback.id)} again.`, `${rollback.id}|`, serving.id),
                            )}
                        {/if}
                    {/if}
                    <p class="soft">{name} keeps the serving release and one rollback. Layers that did not change share their files.</p>
                </section>
            {/if}
        </div>
    </div>
{/if}

<style>
    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(min(100%, 420px), 1fr));
        gap: 16px;
        align-items: start;
    }
    .column {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }
    .lock,
    .done {
        border: 2px solid var(--ink);
    }
    .confirm {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 16px;
        background: #fdf3e6;
        border: 1px solid var(--stale);
        border-radius: 8px;
    }
    .layer,
    .release {
        display: flex;
        align-items: baseline;
        gap: 12px;
        padding: 6px 0;
        border-bottom: 1px solid var(--line);
    }
    .name {
        flex: 1;
        display: flex;
        flex-direction: column;
        min-width: 0;
    }
    .name > span {
        font-size: 13px;
    }
    .release {
        flex-wrap: wrap;
        align-items: center;
    }
    .release span:first-of-type {
        flex: 1;
    }
    .pill {
        padding: 2px 10px;
        background: var(--ink);
        color: var(--panel);
        border-radius: 999px;
        font-size: 13px;
    }
    .locked {
        display: inline-flex;
        align-items: center;
        align-self: flex-start;
        min-height: 44px;
        padding: 0 20px;
        border-radius: 8px;
        font-weight: 600;
        opacity: 0.5;
    }
</style>
