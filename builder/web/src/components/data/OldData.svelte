<script lang="ts">
    import type { CliError } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import { blocking, count, size, type Check, type Legacy, type RunList } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";

    // The keys of the legacy publish path that the target does not use. They go only once it serves
    // a release of `obc data`, after a confirm with the digest of the list that the person saw, in a
    // run of their own: one at a time. `error` is why the checks of a publish cannot be read.
    let {
        name,
        serving,
        publish,
        error,
        runs,
    }: { name: string; serving: boolean; publish: Check[] | null; error: CliError | null; runs: CliQuery<RunList> } = $props();

    const REASON = "Legacy publish path: the target serves releases of obc data";
    const blocked = $derived(publish && blocking(publish));
    // A boolean, so that only a change of it lists the keys again.
    const ready = $derived(serving && !!publish && !blocked);
    const legacy = $derived(ready ? new CliQuery<Legacy>(["r2", "legacy", name]) : null);
    $effect(() => void legacy?.refresh());
    const folders = $derived(Object.entries(legacy?.output?.folders ?? {}));
    const keys = $derived(folders.reduce((sum, [, keys]) => sum + keys.length, 0));
    const bytes = $derived(folders.reduce((sum, [, keys]) => sum + keys.reduce((all, key) => all + key.bytes, 0), 0));
    const deleting = $derived(
        runs.output?.runs.find(
            (run) => run.args.slice(0, 3).join(" ") === `r2 legacy ${name}` && (run.state === "starting" || run.state === "running"),
        ) ?? null,
    );
    let asking = $state(false);
    // The run that this page started counts until the list of runs shows it.
    let started = $state<string | null>(null);
    const running = $derived(deleting?.id ?? (started && !runs.output?.runs.some((run) => run.id === started) ? started : null));
</script>

<section class="card" aria-labelledby="old-data">
    <div class="title">
        <h2 id="old-data">Old data</h2>
        <span class="grow"></span>
        {#if legacy}<Action label="List again" query={legacy} />{/if}
    </div>
    {#if !serving}
        <p class="soft">The keys of the legacy publish path stay until {name} serves a release of obc data.</p>
    {:else if error}
        <Failure {error} />
    {:else if !publish}
        <p class="soft">Checking what a delete needs…</p>
    {:else if blocked}
        <p class="missing">Listing and deleting them needs {blocked.name}: {blocked.fix}</p>
    {:else if legacy?.error}
        <Failure error={legacy.error} />
    {:else if !legacy?.output}
        <p class="soft">Listing the keys under cell-catalog/ and planner/…</p>
    {:else if keys === 0}
        <p>✓ No key of the legacy publish path is left.</p>
    {:else}
        <p><b>{count(keys, "key")}, {size(bytes)}</b> of the legacy publish path, which {name} does not use:</p>
        {#each folders as [folder, listed] (folder)}<p class="soft">{folder}/ · {count(listed.length, "key")}</p>{/each}
        {#if running}
            <p>Run <a href="#/runs/{running}">{running}</a> deletes them. List again when it ends.</p>
        {:else if asking}
            <div class="confirm">
                <b>Delete {count(keys, "key")} from {name}?</b>
                <p>This cannot be undone. removed.jsonl in the bucket keeps each key and the reason.</p>
                <div class="title">
                    <Action
                        label="Delete now"
                        args={["r2", "legacy", name, "--delete", "--expect", legacy.output.digest, "--reason", REASON, "--yes"]}
                        ondone={(run: { run: string }) => {
                            asking = false;
                            started = run.run;
                            void runs.refresh();
                        }}
                    />
                    <button type="button" onclick={() => (asking = false)}>Cancel</button>
                </div>
            </div>
        {:else}
            <button type="button" onclick={() => (asking = true)}>Delete old data…</button>
        {/if}
    {/if}
</section>

<style>
    .confirm {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 16px;
        background: var(--warn);
        border: 1px solid var(--stale);
        border-radius: 8px;
    }
</style>
