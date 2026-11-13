<script lang="ts">
    import { onMount } from 'svelte';
    import { Sha256 } from '../../lib/core/hash';
    import { armUpdate, stageFirmware } from '../../lib/device/write';
    import { firmwareCheck } from '../../lib/firmware/check.svelte';
    import { updateStatus } from '../../lib/firmware/release';
    import { DeviceError, type FlatStoreClient } from '../../lib/usb/client';

    /** The value and the action of the row Software. `busy`: a sync uses the client, and transfers do not queue. */
    let { client, running, busy }: { client: FlatStoreClient; running: string | null; busy: boolean } = $props();

    // The check asks the update host only while a device is connected; this row shows only then.
    onMount(() => void firmwareCheck.ensure());

    const release = $derived(firmwareCheck.release);
    const status = $derived(updateStatus(running, release?.version ?? null));
    let step = $state<string | null>(null);
    let said = $state<string | null>(null);

    /** The two steps of the map builder: the package goes to the card, then the device is asked to install it. Only the
     * answer of the device says that it installs. */
    async function update() {
        const target = release;
        if (!target || step) return;
        said = null;
        const controller = new AbortController();
        const ctx = { signal: controller.signal, cancel: (reason?: unknown) => controller.abort(reason), phase: () => {}, progress: () => {} };
        try {
            step = `Sending version ${target.version} to the device`;
            const response = await fetch(target.url, { credentials: 'omit' });
            if (!response.ok) throw new Error(`The update could not be downloaded (HTTP ${response.status}).`);
            const bytes = new Uint8Array(await response.arrayBuffer());
            if (bytes.length !== target.bytes || Sha256.hex(bytes) !== target.sha256) throw new Error('The downloaded update failed its check. Nothing went to the device.');
            const { result } = await stageFirmware(client, bytes, ctx);
            step = 'Asking the device to install it';
            await armUpdate(client, result);
            said = `The device installs version ${target.version} when it restarts. Keep it powered.`;
        } catch (cause) {
            said = cause instanceof DeviceError && cause.code === 'rejected'
                ? `The device refused to install version ${target.version}. The update is on its memory card, and the device runs version ${running} as before.`
                : cause instanceof Error ? cause.message : String(cause);
        } finally {
            step = null;
        }
    }
</script>

<div>
    {#if status === 'available' && release}
        <p>Version {release.version} is ready</p>
        <p class="second">The device has version {running}</p>
    {:else if status === 'current'}
        <p>Version {running} · Up to date</p>
    {:else if status === 'unknown' && running}
        <p>Development build <span class="mono">{running}</span></p>
        <p class="second">Automatic updates are paused.</p>
    {:else if status === 'unknown'}
        <p>The device did not report its version.</p>
    {:else}
        <p>Version {running}</p>
        {#if status === 'ahead' && release}<p class="second">Newer than the published version {release.version}</p>
        {:else if firmwareCheck.failed}<p class="second">The check for a new version did not answer.</p>{/if}
    {/if}
    {#if step}<p class="second">{step}</p>{/if}
    {#if said}<p class="second" role="status">{said}</p>{/if}
</div>
<div>
    {#if status === 'available' && release}
        <button type="button" class="update" onclick={update} disabled={busy || !!step}>Update</button>
    {/if}
</div>

<style>
    p {
        margin: 0;
    }
    .second {
        margin-top: 4px;
        color: var(--ink-soft);
        font-size: 13.5px;
    }
    .mono {
        font-family: var(--mono);
    }
    .update {
        min-height: 34px;
        padding: 5px 14px;
        border: 1px solid var(--rust);
        border-radius: 6px;
        background: var(--rust);
        color: var(--cream);
    }
    .update:disabled {
        opacity: .6;
    }
</style>
