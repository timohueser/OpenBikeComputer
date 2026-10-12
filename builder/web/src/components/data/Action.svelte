<script lang="ts" generics="T">
    import { command, runCli, type CliError } from "../../lib/data/cli";
    import type { CliQuery } from "../../lib/data/query.svelte";
    import Failure from "./Failure.svelte";

    // The page can do only what the CLI can: a button runs exactly the command it shows.
    let {
        args,
        label,
        ondone,
        query,
    }: { label: string; ondone?: (output: T) => void } & (
        | { args: readonly string[]; query?: never }
        /** The Action runs the args of the query and gives it the answer; the view of the query
         *  shows the error. */
        | { query: CliQuery<T>; args?: never }
    ) = $props();
    const argv = $derived((query?.args ?? args) as readonly string[]);

    let running = $state(false);
    let error = $state<CliError | null>(null);

    async function run() {
        running = true;
        error = null;
        const take = query?.ask();
        const result = await runCli<T>(argv);
        running = false;
        take?.(result);
        if (result.ok) ondone?.(result.output);
        else if (!query) error = result.error;
    }
</script>

<div class="action">
    <button type="button" disabled={running} aria-busy={running} onclick={run}>{label}</button>
    <code>{command(argv)}</code>
    {#if error}<Failure {error} />{/if}
</div>

<style>
    .action {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 12px;
        min-width: 0;
    }
    code {
        min-width: 0;
        padding: 8px 10px;
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 6px;
    }
    .action > :global(.failure) {
        flex-basis: 100%;
    }
</style>
