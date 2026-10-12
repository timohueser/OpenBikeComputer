<script lang="ts">
    import { flagsOf } from "../../lib/data/flags";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { targetAt } from "../../lib/data/screens";
    import { clock, LAYER_MARK, short, when, type RunView, type Status } from "../../lib/data/shapes";
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

    const stale = $derived(target?.layers?.filter((layer) => layer.state === "stale") ?? []);
    const causes = $derived([...new Set(stale.map((layer) => layer.text))].join(" · "));
    // The page asks before it runs a command that changes an R2 target, so it sends `--yes`.
    const yes = $derived(target?.kind === "r2" ? ["--yes"] : []);
    let asking = $state<"rollback" | "unlock" | null>(null);
    let done = $state("");
    function changed(text: string) {
        asking = null;
        done = text;
        void status.refresh();
    }
</script>

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
        <Review {target} flags={[]} {machine} />
    {:else if resumed?.output}
        <p class="soft">This review resumes run <a href="#/runs/{route.resume}">{route.resume}</a> with its settings.</p>
        <Review {target} flags={flagsOf(resumed.output.args)} {machine} />
    {/if}
{:else}
    {#if target.error}<Failure error={{ code: "failed", message: target.error, fix: target.fix ?? "" }} />{/if}
    {#if target.lock_error}<p class="missing">The lock of {name} cannot be read: {target.lock_error}</p>{/if}
    {#if done}<p class="ok" role="status">{done}</p>{/if}

    {#if target.lock}
        {@const lock = target.lock}
        <section class="card lock" aria-labelledby="lock">
            <h2 id="lock">Locked by run <a href="#/runs/{lock.run}">{lock.run}</a></h2>
            <p>On {lock.machine} · phase {lock.phase} · last heartbeat {clock(lock.heartbeat)}</p>
            {#if !lock.stale}
                <p class="soft">An update of {name} waits until this run ends.</p>
            {:else if asking === "unlock"}
                <div class="confirm">
                    <b>Take over the {name} lock from {lock.machine}?</b>
                    <p>If that run still runs, it sees that it lost the lock before it switches {name}, and stops. {name} never gets two updates at once.</p>
                    <p>Its finished layers stay on {lock.machine}.</p>
                    <div class="title">
                        <Action
                            label="Take over the lock"
                            args={["unlock", name, "--from", lock.machine, "--yes"]}
                            ondone={() => changed(`The ${name} lock is free.`)}
                        />
                        <button type="button" onclick={() => (asking = null)}>Cancel</button>
                    </div>
                </div>
            {:else}
                <p class="missing">No heartbeat since {when(lock.heartbeat)}: the run likely ended without freeing the lock.</p>
                <button type="button" onclick={() => (asking = "unlock")}>Take over the lock…</button>
            {/if}
        </section>
    {/if}

    <div class="grid">
        <section class="card" aria-labelledby="layers">
            <h2 id="layers">Layers{target.region ? ` of ${target.region}` : ""}</h2>
            {#each target.layers ?? [] as layer (layer.layer)}
                <div class="layer">
                    <span class={layer.state} aria-hidden="true">{LAYER_MARK[layer.state]}</span>
                    <span class="name"><b>{layer.layer}</b><span class={layer.state === "ok" ? "soft" : layer.state}>{layer.text ?? (layer.state === "off" ? "off" : "up to date")}</span></span>
                    {#if layer.format !== null}<code>{layer.format}.{layer.revision}</code>{/if}
                </div>
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
                    <p><b class="stale">{stale.length} layers stale.</b> Causes: {causes}.</p>
                {:else if target.layers}
                    <p>Each layer is up to date.</p>
                {/if}
                {#if target.drift.length}<p class="missing">These keys differ from the serving release: {target.drift.join(", ")}</p>{/if}
                {#if target.next}<p class="soft">Next: <code>{target.next}</code></p>{/if}
                <a class="primary" href="#/{name}/review">Review update</a>
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
                                <button type="button" onclick={() => (asking = "rollback")}>Roll back to {short(rollback.id)}…</button>
                            {/if}
                        </div>
                        {#if asking === "rollback"}
                            <div class="confirm">
                                <b>Roll back {name} to {short(rollback.id)}?</b>
                                <p>{name} serves {short(rollback.id)} instead of {short(serving.id)}. Nothing is deleted: {short(serving.id)} becomes the rollback.</p>
                                <div class="title">
                                    <Action
                                        label="Roll back now"
                                        args={["rollback", name, ...yes]}
                                        ondone={() => changed(`${name} serves ${short(rollback.id)} now.`)}
                                    />
                                    <button type="button" onclick={() => (asking = null)}>Cancel</button>
                                </div>
                            </div>
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
    .lock {
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
</style>
