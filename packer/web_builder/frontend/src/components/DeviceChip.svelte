<!--
  The header's device chip: connection state, connect/disconnect, and a readout for whichever
  transfer is running — global chrome for what used to live inside one page's step 4.

  It renders only on tiers with `caps.deviceDashboard`, where the device is a place you *go*
  (the Device tab) rather than a step you scroll to; the label doubles as the link there. The
  session itself is the same shared `deviceHolder` every surface uses — opening here is what makes
  "plug it in and the app lights up" true on every tab, not just the one that used to own the
  connect button.

  Entry-chunk discipline: this component is part of the header, which is part of the entry bundle,
  so it must not import anything from `lib/usb` at runtime (`usb/bundle.test.ts`). `deviceHolder`
  is safe — its usb imports are type-only — and the transport arrives through `platform.device`'s
  own dynamic import, exactly as before.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import { jobRegistry } from "../lib/device/job.svelte";
    import { deviceHolder } from "../lib/device/session.svelte";
    import { DEVICE_ROUTE } from "../lib/routes";

    // Adopt an already-attached device with no prompt; the watcher keeps the chip
    // green/grey from here on. Memoized, so DeviceStep's own call stays harmless.
    onMount(() => void deviceHolder.open());

    const session = $derived(deviceHolder.session);
    let prompting = $state(false);

    function connect() {
        const current = session;
        if (!current) return;
        prompting = true;
        deviceHolder.interrupted = null;
        // No await before the call: WebUSB's chooser must open from the click's own
        // stack. Native transports don't care, so the strict form serves both.
        void current.requestDevice().finally(() => (prompting = false));
    }

    const active = $derived(jobRegistry.active);
</script>

<div class="chip-wrap">
    {#if active}
        <span class="job small faint" title="{active.pct}%">
            sending {active.label}{#if active.total > 0}
                · {active.pct}%{/if}
        </span>
    {/if}

    {#if session?.status === "ready"}
        <span class="chip">
            <span class="dot ok" aria-hidden="true"></span>
            <a href={DEVICE_ROUTE} class="label">
                OBC
                {#if session.info}<span class="mono faint">{session.info.hardwareRevision}</span>{/if}
            </a>
            <button type="button" class="link" onclick={() => session && void session.disconnect()}>
                Disconnect
            </button>
        </span>
    {:else if session?.status === "connecting"}
        <span class="chip">
            <span class="dot busy" aria-hidden="true"></span>
            <span class="label">Connecting…</span>
        </span>
    {:else if session?.status === "unsupported"}
        <span class="chip" title={session.error ?? undefined}>
            <span class="dot off" aria-hidden="true"></span>
            <span class="label faint">USB unavailable</span>
        </span>
    {:else}
        <span class="chip" title={deviceHolder.error ?? session?.error ?? undefined}>
            <span class="dot off" aria-hidden="true"></span>
            <a href={DEVICE_ROUTE} class="label faint">No device</a>
            <button type="button" class="link" disabled={prompting || !session} onclick={connect}>
                Connect
            </button>
        </span>
    {/if}
</div>

<style>
    .chip-wrap {
        display: flex;
        align-items: center;
        gap: 10px;
        min-width: 0;
    }

    .job {
        white-space: nowrap;
        font-variant-numeric: tabular-nums;
    }

    .chip {
        display: inline-flex;
        align-items: center;
        gap: 7px;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        padding: 3px 12px;
        background: var(--panel);
        font-size: 12.5px;
        white-space: nowrap;
    }

    .dot {
        width: 7px;
        height: 7px;
        border-radius: 50%;
        flex: none;
    }

    .dot.ok {
        background: var(--forest);
    }

    .dot.busy {
        background: var(--amber);
    }

    .dot.off {
        background: transparent;
        border: 1.4px solid var(--ink-faint);
    }

    .label {
        display: inline-flex;
        align-items: baseline;
        gap: 6px;
        color: var(--ink);
        text-decoration: none;
    }

    .label.faint {
        color: var(--ink-faint);
    }

    .label:hover {
        text-decoration: underline;
    }

    .mono {
        font-family: var(--mono);
        font-size: 11px;
    }

    .link {
        border: 0;
        background: none;
        padding: 0;
        font: inherit;
        font-size: 12px;
        color: var(--ink-soft);
        text-decoration: underline;
        text-decoration-style: dotted;
        cursor: pointer;
    }

    .link:disabled {
        opacity: 0.5;
        cursor: default;
    }

    .faint {
        color: var(--ink-faint);
    }

    .small {
        font-size: 12px;
    }
</style>
