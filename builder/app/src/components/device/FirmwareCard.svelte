<!--
  Firmware, over the same cable as everything else.

  Two steps, and they stay two. **Staging** is an ordinary `PUT` of an update-package object (§4's
  kind 7): the verified `UPDATE.BIN` lands on the card and nothing else happens. **Arming** is the
  separate `ARM` request that makes a staged package the next boot, and it is the only thing on this
  card that changes what the device will run.

  The device's current policy **refuses to arm**, answering `rejected` (a stated dev-window gap).
  The button is wired and drawn all the same: a refusal a rider can read is worth more than an
  affordance that quietly does nothing, and worth far more than a card that pretends the update
  installed. So the copy here never says "installing" unless the device said so first.

  The version check is #773's: an anonymous GET for the published manifest, compared against the
  firmware revision §5.2.1's device-info payload reports. A device running a probe-flashed build
  reports a git hash rather than a version — that parses as nothing, and no update is offered,
  deliberately. The card says so in as many words ("development build — automatic updates are
  paused") rather than going quiet, and the file picker below stays the way such a device gets back
  onto releases.

  The check itself is `lib/firmware/check.svelte.ts`, shared with the prompt that appears when a
  device connects (#1002). This card remains the only surface that stages or arms anything; the
  prompt can only point here.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import { formatBytes } from "../../lib/format";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { armUpdate, stageFirmware } from "../../lib/device/write";
    import { firmwareCheck } from "../../lib/firmware/check.svelte";
    import { updateStatus, type FirmwareRelease } from "../../lib/firmware/release";
    import { FIRMWARE_ANCHOR } from "../../lib/routes";
    import { Sha256 } from "../../lib/device/sha256";
    import { DeviceError, type FlatStoreClient } from "../../lib/usb/client";
    import type { DeviceInfo } from "../../lib/usb/records";
    import TransferBar from "./TransferBar.svelte";

    let { client, info }: { client: FlatStoreClient; info: DeviceInfo | null } = $props();

    const job = new DeviceJob("firmware");
    /** The package this card put on the card: its version, and the `(ObjectId, Revision)` the
     *  commit published — which is what `ARM`'s compare-and-swap names (§4). */
    let staged = $state<{ version: string; objectId: bigint; revision: bigint } | null>(null);
    let armed = $state(false);
    let armError = $state<string | null>(null);
    let arming = $state(false);
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
        reset();
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
        }, describeStaged);
    }

    async function sendFile(file: File) {
        reset();
        await job.run(async (ctx) => {
            ctx.phase("reading", file.size);
            return stageFirmware(client, new Uint8Array(await file.arrayBuffer()), ctx);
        }, describeStaged);
    }

    function reset() {
        staged = null;
        armed = false;
        armError = null;
    }

    function describeStaged(result: Awaited<ReturnType<typeof stageFirmware>>): string {
        staged = { version: result.image.version, objectId: result.result.objectId, revision: result.result.revision };
        return `${result.image.version} is on the device's card (${formatBytes(result.image.containerLen)}).`;
    }

    function onPick(event: Event) {
        const input = event.currentTarget as HTMLInputElement;
        const file = input.files?.[0];
        input.value = "";
        if (file) void sendFile(file);
    }

    /**
     * Ask the device to make the staged package its next boot (§4's `ARM`).
     *
     * A `rejected` answer is the device's stated policy rather than a fault, so it gets its own
     * sentence: the rider is told the device refused, not that something broke. Any other failure
     * keeps the client's own message.
     */
    async function arm() {
        const target = staged;
        if (!target || arming) return;
        arming = true;
        armError = null;
        try {
            await armUpdate(client, target);
            armed = true;
        } catch (cause) {
            armError =
                cause instanceof DeviceError && cause.code === "rejected"
                    ? `The device refused to arm this update. ${target.version} is still on its card, ` +
                      "and the device is running what it was running before."
                    : cause instanceof Error
                      ? cause.message
                      : String(cause);
        } finally {
            arming = false;
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

    {#if staged && !armed}
        <div class="confirm">
            <button type="button" class="btn primary" disabled={arming} onclick={() => void arm()}>
                Install {staged.version} on next boot
            </button>
            <p class="small faint">The image is on the card either way; this is what makes it the one that boots.</p>
        </div>
    {/if}

    {#if armed && staged}
        <p class="note small">
            {staged.version} is armed. The device reboots and installs it on its own — keep it powered.
        </p>
    {/if}

    {#if armError}
        <p class="note error small" role="alert">{armError}</p>
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
