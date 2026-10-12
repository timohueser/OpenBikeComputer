<script lang="ts">
    import { MARK, PHASES, type Phase, type RunState } from "../../lib/data/shapes";

    // `entered` holds each phase that the run entered, in order; the phases after them wait.
    let { entered }: { entered: { phase: Phase; state: RunState; waiting?: string }[] } = $props();

    const shown = $derived(PHASES.map((phase) => ({ phase, ...entered.find((at) => at.phase === phase) })));
</script>

<ol class="phases" aria-label="Phases">
    {#each shown as { phase, state, waiting } (phase)}
        <li class={state ?? "waits"}>
            <span aria-hidden="true">{state ? MARK[state] : "○"}</span>
            {phase}
            <span class="hidden">{state ?? "waits"}</span>
            {#if waiting}<span class="why">: {waiting}</span>{/if}
        </li>
    {/each}
</ol>

<style>
    .phases {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
        margin: 0;
        padding: 16px;
        list-style: none;
        background: var(--panel);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
    }
    li {
        padding: 6px 14px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        font-weight: 600;
        text-transform: capitalize;
    }
    .why {
        font-weight: 400;
        text-transform: none;
    }
    .waits {
        color: var(--ink-faint);
        background: var(--parchment);
    }
    .starting,
    .running,
    .failed,
    .stopped {
        border: 2px solid;
    }
    .hidden {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
    }
</style>
