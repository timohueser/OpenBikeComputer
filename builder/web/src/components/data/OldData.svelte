<script lang="ts">
    import { CliQuery } from "../../lib/data/query.svelte";
    import { blocking, count, size, type Check, type Legacy } from "../../lib/data/shapes";
    import Action from "./Action.svelte";
    import Failure from "./Failure.svelte";

    // The keys of the legacy publish path that the target does not use. They go only once it serves
    // a release of `obc data`, after a confirm with the digest of the list that the person saw.
    let { name, serving, publish }: { name: string; serving: boolean; publish: Check[] | null } = $props();

    const REASON = "Legacy publish path: the target serves releases of obc data";
    const blocked = $derived(publish && blocking(publish));
    // A boolean, so that only a change of it lists the keys again.
    const ready = $derived(serving && !!publish && !blocked);
    const legacy = $derived(ready ? new CliQuery<Legacy>(["r2", "legacy", name]) : null);
    $effect(() => void legacy?.refresh());
    const folders = $derived(Object.entries(legacy?.output?.folders ?? {}));
    const keys = $derived(folders.reduce((sum, [, keys]) => sum + keys.length, 0));
    const bytes = $derived(folders.reduce((sum, [, keys]) => sum + keys.reduce((all, key) => all + key.bytes, 0), 0));
    let asking = $state(false);
</script>

<section class="card" aria-labelledby="old-data">
    <h2 id="old-data">Old data</h2>
    {#if !serving}
        <p class="soft">The keys of the legacy publish path stay until {name} serves a release of obc data.</p>
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
        {#if asking}
            <div class="confirm">
                <b>Delete {count(keys, "key")} from {name}?</b>
                <p>This cannot be undone. removed.jsonl in the bucket keeps each key and the reason.</p>
                <div class="title">
                    <Action
                        label="Delete now"
                        args={["r2", "legacy", name, "--delete", "--expect", legacy.output.digest, "--reason", REASON, "--yes"]}
                        ondone={() => {
                            asking = false;
                            void legacy?.refresh();
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
        background: #fdf3e6;
        border: 1px solid var(--stale);
        border-radius: 8px;
    }
</style>
