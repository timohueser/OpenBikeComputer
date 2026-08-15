<!--
  A device write in progress: what phase, how far, how fast, how much longer, and a Cancel.

  The rate and the estimate are here because of one fact about this hardware: a regional map is
  hundreds of megabytes, so the transfer is minutes however fast the slowest stage of the pipeline
  turns out to be. A bare percentage invites "it's stuck at 12%"; a number of MB/s and a remaining
  time say what is happening, and the phase says which half of the job it is happening in.
-->
<script lang="ts">
    import { formatBytes, formatDuration, formatRate } from "../../lib/format";
    import type { DeviceJob } from "../../lib/device/job.svelte";

    let { job, label }: { job: DeviceJob; label?: string } = $props();

    const PHASES: Record<string, string> = {
        reading: "Reading the file",
        downloading: "Downloading",
        verifying: "Checking the file",
        converting: "Converting to GPX",
        assembling: "Assembling the map",
        sending: "Writing to the device",
        committing: "Finishing on the device",
    };

    const heading = $derived(PHASES[job.phase] ?? label ?? "Working");
    const eta = $derived(job.etaSeconds);
</script>

{#if job.running}
    <div class="transfer">
        <div class="bar"><div class="fill" style:width="{job.pct}%"></div></div>
        <div class="line small">
            <span class="muted">{heading}</span>
            <span class="faint">
                {job.partTotal
                    ? `${job.partLabel ?? `shard ${job.partCurrent} of ${job.partTotal}`} · ${job.pct}% · `
                    : ""}
                {formatBytes(job.done)}{job.total ? ` of ${formatBytes(job.total)}` : ""}{job.rate
                    ? ` · ${formatRate(job.rate)}`
                    : ""}{eta ? ` · about ${formatDuration(eta)} left` : ""}
            </span>
            <button type="button" class="btn ghost" onclick={() => job.cancel()}>Cancel</button>
        </div>
    </div>
{/if}

{#if job.phase === "error" && job.error}
    <p class="note error small" role="alert">{job.error}</p>
{/if}

{#if job.phase === "done" && job.result}
    <p class="note small done">{job.result}</p>
{/if}

<style>
    .transfer {
        margin-top: 10px;
    }

    .bar {
        height: 6px;
        border-radius: 3px;
        background: var(--parchment-3);
        overflow: hidden;
    }

    .fill {
        height: 100%;
        background: var(--forest);
        transition: width 0.2s linear;
    }

    .line {
        display: flex;
        align-items: baseline;
        gap: 8px;
        margin-top: 6px;
    }

    .line .faint {
        margin-left: auto;
        font-variant-numeric: tabular-nums;
    }

    .line .btn {
        padding: 2px 8px;
        font-size: 12px;
    }

    .note {
        margin: 8px 0 0;
    }

    .error {
        color: var(--coral);
    }

    .done {
        color: var(--forest-deep);
    }
</style>
