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
    }: {
        args: readonly string[];
        label: string;
        ondone?: (output: T) => void;
        /** A query of the same args. It takes the answer, and its view shows the error. */
        query?: CliQuery<T>;
    } = $props();

    let running = $state(false);
    let error = $state<CliError | null>(null);

    async function run() {
        running = true;
        error = null;
        const take = query?.ask();
        const result = await runCli<T>(args);
        running = false;
        take?.(result);
        if (result.ok) ondone?.(result.output);
        else if (!query) error = result.error;
    }
</script>

<div class="action">
    <button type="button" disabled={running} aria-busy={running} onclick={run}>{label}</button>
    <code>{command(args)}</code>
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
    button {
        min-height: 44px;
        padding: 0 20px;
        background: var(--panel);
        color: var(--ink);
        border: 1px solid var(--ink);
        border-radius: 8px;
        font-size: 15px;
        font-weight: 600;
    }
    button:disabled {
        opacity: 0.6;
        cursor: progress;
    }
    code {
        min-width: 0;
        padding: 8px 10px;
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        font-family: var(--mono);
        font-size: 13px;
        overflow-wrap: anywhere;
    }
    .action > :global(.failure) {
        flex-basis: 100%;
    }
</style>
