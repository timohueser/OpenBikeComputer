<script lang="ts">
    import type { Snippet } from "svelte";
    import { command, type CliError } from "../../lib/data/cli";

    // `children` adds lines below the fix, such as what a failed run kept.
    let { error, children }: { error: CliError; children?: Snippet } = $props();
</script>

<!-- The CLI marks commands and paths with backticks; odd parts of a split are code. -->
{#snippet text(value: string)}
    {#each value.split("`") as part, i}{#if i % 2}<code>{part}</code>{:else}{part}{/if}{/each}
{/snippet}

<section class="failure" role="alert">
    <p class="message">✗ {@render text(error.message)}</p>
    <p><b>Fix</b> · {@render text(error.fix)}</p>
    {#if error.run}<p><b>Run</b> · <code>{command(["runs", error.run])}</code></p>{/if}
    {@render children?.()}
</section>

<style>
    .failure {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 16px;
        background: var(--panel);
        border: 2px solid var(--missing);
        border-radius: 12px;
        overflow-wrap: anywhere;
    }
    p {
        margin: 0;
    }
    .message {
        font-weight: 600;
        color: var(--missing);
    }
    code {
        font-family: var(--mono);
        font-size: 13px;
    }
</style>
