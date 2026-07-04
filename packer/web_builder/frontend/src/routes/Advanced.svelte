<script lang="ts">
    import { working } from "../lib/config/storage.svelte";

    const basedOn = $derived(working.envelope?.based_on?.id ?? null);
    const modified = $derived(working.envelope?.modified ?? false);
</script>

<div class="head">
    <a href="#/" class="small">← Map builder</a>
    <h2>Advanced editor</h2>
    {#if basedOn}
        <span class="badge small">
            {modified ? `Custom — based on ${basedOn}` : `Preset: ${basedOn}`}
        </span>
    {/if}
</div>

<div class="card placeholder">
    <p>
        The full editor — feature styling, detail levels, and output settings — lands here in the
        next part of the redesign.
    </p>
    <p class="muted small">
        Until then the previous editor is still available at <a href="/legacy">/legacy</a> (it edits
        a server-side config, separate from your preset choice here).
    </p>
</div>

<style>
    .head {
        display: flex;
        align-items: baseline;
        gap: 14px;
        margin-bottom: 14px;
        flex-wrap: wrap;
    }

    .head h2 {
        font-size: 22px;
    }

    .badge {
        background: rgba(227, 173, 51, 0.28);
        border: 1px solid var(--amber);
        border-radius: 999px;
        padding: 2px 10px;
        font-weight: 600;
    }

    .placeholder p {
        margin: 0 0 8px;
    }
</style>
