<script lang="ts">
    import type { Check } from "../../lib/data/shapes";

    // What an action needs: each check that passes on one line, and each that fails with its problem,
    // its fix and the commands of the fix, each with a copy button.
    let { checks }: { checks: Check[] } = $props();

    let copied = $state<string | null>(null);
    async function copy(command: string) {
        await navigator.clipboard?.writeText(command);
        copied = command;
    }
</script>

<ul class="checks">
    {#each checks as check (check.name)}
        <li>
            <span class={check.problem ? (check.blocks ? "missing" : "stale") : "ready"} aria-hidden="true">{check.problem ? "✗" : "✓"}</span>
            <div>
                <b>{check.name}</b>{#if check.problem}<span class="soft">{` · ${check.problem}`}</span>{/if}
                {#if check.fix}<p>{check.fix}</p>{/if}
                {#each check.commands as command (command)}
                    <div class="command">
                        <code>{command}</code>
                        <button type="button" onclick={() => copy(command)}>{copied === command ? "Copied" : "Copy"}</button>
                    </div>
                {/each}
            </div>
        </li>
    {/each}
</ul>

<style>
    .checks {
        display: flex;
        flex-direction: column;
        gap: 6px;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: flex;
        gap: 10px;
        align-items: baseline;
    }
    li > div {
        display: flex;
        flex-direction: column;
        gap: 4px;
        min-width: 0;
        overflow-wrap: anywhere;
    }
    p {
        margin: 0;
    }
    .command {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 6px 8px;
    }
    .command code {
        flex: 1 1 240px;
        min-width: 0;
        padding: 6px 8px;
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 6px;
    }
</style>
