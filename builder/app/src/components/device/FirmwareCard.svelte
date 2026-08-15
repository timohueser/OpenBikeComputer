<!--
  Firmware, over the same cable as everything else.

  DFU here is *SD-staged*: the page writes an `UPDATE.BIN` to the card and that is all it does.
  Installing is the device's own confirm card and a physical Select press — a link may stage an
  image and can never arm one, which is the same rule the phone has lived under since #615. So the
  copy on this card never says "installing", and the two steps stay two steps.

  The version check is #773's: an anonymous GET for the published manifest, compared against the
  running version the identity/DIS read reports. A device running a probe-flashed build reports a
  git hash rather than a version — that parses as nothing, and no update is offered, deliberately.
  The card says so in as many words ("development build — automatic updates are paused") rather
  than going quiet, and the file picker below stays the way such a device gets back onto releases.

  The check itself is `lib/firmware/check.svelte.ts`, shared with the prompt that appears when a
  device connects (#1002). This card remains the only surface that stages anything or asks the
  device to install it; the prompt can only point here.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import { formatBytes } from "../../lib/format";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { askToInstall, stageFirmware } from "../../lib/device/write";
    import { firmwareCheck } from "../../lib/firmware/check.svelte";
    import { updateStatus, type FirmwareRelease } from "../../lib/firmware/release";
    import { FIRMWARE_ANCHOR } from "../../lib/routes";
    import { Sha256 } from "../../lib/device/sha256";
    import type { ProtocolClient } from "../../lib/usb/client";
    import type { DeviceInfo } from "../../lib/usb/transport";
    import TransferBar from "./TransferBar.svelte";

    let { client, info }: { client: ProtocolClient; info: DeviceInfo | null } = $props();

    const job = new DeviceJob("firmware");
    let staged = $state<string | null>(null);
    let asked = $state(false);
    let askError = $state<string | null>(null);
    let picker = $state<HTMLInputElement>();

    // Only once a device is connected: with nothing to compare against, a request to the update
    // host buys the visitor nothing and costs them a connection they did not ask for. This card
    // only ever renders behind a live client, so mounting *is* that condition — and the shared
    // check makes at most one request however many surfaces ask.
    onMount(() => void firmwareCheck.ensure());

    const release = $derived(firmwareCheck.release);
    const checkFailed = $derived(firmwareCheck.failed);
    const running = $derived(info?.firmwareRevision ?? null);
    const status = $derived(updateStatus(running, release?.version ?? null));

    async function sendRelease(entry: FirmwareRelease) {
        // Acting on it answers the question the prompt would otherwise ask about this version.
        firmwareCheck.answer(info?.serialNumber, entry.version);
        staged = null;
        asked = false;
        askError = null;
        await job.run(async (ctx) => {
            ctx.phase("downloading", entry.bytes);
            const response = await fetch(entry.url, { signal: ctx.signal, credentials: "omit" });
            if (!response.ok) throw new Error(`The update could not be downloaded (HTTP ${response.status}).`);
            // An update image is ~1.5 MB at most (`OBCU_Spec.md` §1.1's slot ceiling), so it is
            // read whole — the streaming machinery a map needs would be noise at this size.
            const bytes = new Uint8Array(await response.arrayBuffer());
            if (bytes.length !== entry.bytes || Sha256.hex(bytes) !== entry.sha256) {
                throw new Error("The downloaded update failed its checksum. Nothing was sent to the device.");
            }
            return stageFirmware(client, bytes, ctx);
        }, (result) => {
            staged = result.image.version;
            return `${result.image.version} is on the device's card (${formatBytes(result.image.containerLen)}).`;
        });
    }

    async function sendFile(file: File) {
        staged = null;
        asked = false;
        askError = null;
        await job.run(async (ctx) => {
            ctx.phase("reading", file.size);
            return stageFirmware(client, new Uint8Array(await file.arrayBuffer()), ctx);
        }, (result) => {
            staged = result.image.version;
            return `${result.image.version} is on the device's card (${formatBytes(result.image.containerLen)}).`;
        });
    }

    function onPick(event: Event) {
        const input = event.currentTarget as HTMLInputElement;
        const file = input.files?.[0];
        input.value = "";
        if (file) void sendFile(file);
    }

    async function ask() {
        askError = null;
        try {
            await askToInstall(client);
            asked = true;
        } catch (cause) {
            askError = cause instanceof Error ? cause.message : String(cause);
        }
    }
</script>

<!-- The id is where the update prompt scrolls to; `lib/routes.ts` spells it once. -->
<section class="block" id={FIRMWARE_ANCHOR}>
    <h4>Firmware</h4>

    <p class="what small">
        <span class="muted">Running</span>
        <span class="name">{running ?? "unknown"}</span>
    </p>

    {#if status === "available" && release}
        <p class="small">{release.version} is available.</p>
    {:else if status === "current"}
        <p class="small muted">Up to date.</p>
    {:else if status === "ahead" && release}
        <p class="small muted">This device is newer than the published {release.version}.</p>
    {:else if status === "unknown" && running}
        <!-- The third state, said out loud (#773's U4/U5 amendment): the version this device
             reports is not a release version, so the check has nothing it can decide. The picker
             below is how such a device gets back onto the release track. -->
        <p class="small muted">Development build — automatic updates are paused.</p>
    {:else if status === "unknown"}
        <p class="small muted">This device has not reported a firmware version yet.</p>
    {:else if checkFailed}
        <p class="small muted">Couldn't check for updates.</p>
    {:else}
        <p class="small muted">No published update to compare against.</p>
    {/if}

    <div class="actions">
        {#if status === "available" && release}
            <button
                type="button"
                class="btn primary"
                disabled={job.running}
                onclick={() => release && void sendRelease(release)}
            >
                Send {release.version} to device
            </button>
        {/if}
        <button type="button" class="btn" disabled={job.running} onclick={() => picker?.click()}>
            Send an UPDATE.BIN…
        </button>
        <input
            bind:this={picker}
            type="file"
            accept=".bin"
            hidden
            aria-hidden="true"
            tabindex="-1"
            onchange={onPick}
        />
    </div>

    <TransferBar {job} />

    {#if staged && !asked}
        <div class="confirm">
            <button type="button" class="btn primary" onclick={() => void ask()}>
                Ask the device to install it
            </button>
            <p class="small faint">Nothing is installed until you confirm it on the device.</p>
        </div>
    {/if}

    {#if asked}
        <p class="note small">
            The device is asking you to confirm. Press Select on it to install {staged} — it reboots
            and installs on its own. Keep it powered.
        </p>
    {/if}

    {#if askError}
        <p class="note error small" role="alert">{askError}</p>
    {/if}
</section>

<style>
    h4 {
        margin: 0 0 6px;
        font-size: 14px;
        font-family: var(--sans);
        letter-spacing: 0.02em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .what {
        margin: 0 0 4px;
        display: flex;
        gap: 8px;
        align-items: baseline;
    }

    .name {
        font-family: var(--mono);
    }

    p.small {
        margin: 0 0 8px;
    }

    .actions {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }

    .confirm {
        margin-top: 10px;
    }

    .confirm p {
        margin: 6px 0 0;
    }

    .note {
        margin: 8px 0 0;
    }

    .error {
        color: var(--coral);
    }
</style>
