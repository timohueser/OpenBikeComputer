<!--
  "Firmware 1.4.0 is available" — said once, when a device connects (#1002, epic #773).

  Mounted at the app root next to `ConfirmDialog`, and for the same reason: it belongs to no page,
  because a device can be plugged in while the rider is anywhere in the app. Unlike the dialog it is
  **not** modal — there is no backdrop and nothing is blocked. An update is worth mentioning; it is
  never worth stopping someone mid-task over, and a rider who came here to send a map should be able
  to keep sending it.

  It can point and nothing else. Downloading, checksumming, staging and asking the device to install
  all stay in `device/FirmwareCard.svelte`, which is where the rider can see what is happening; this
  scrolls them to it. Both read the one shared check (`lib/firmware/check.svelte.ts`), which is also
  where the "at most once per (device, version)" memory lives.

  Entry-chunk discipline (`platform/bundle.test.ts`): the root is the entry bundle, so nothing here may
  reach `lib/usb` at runtime. `deviceHolder` and `jobRegistry` are already in it — the header's
  device chip imports both — and the check is a plain `fetch` module with no device code in it.
-->
<script lang="ts">
    import { jobRegistry } from "../lib/device/job.svelte";
    import { deviceHolder } from "../lib/device/session.svelte";
    import { firmwareCheck } from "../lib/firmware/check.svelte";
    import { platform } from "../lib/platform";
    import { router } from "../lib/router.svelte";
    import { DEVICE_ROUTE, FIRMWARE_ANCHOR } from "../lib/routes";

    const session = $derived(deviceHolder.session);
    /** Non-null exactly while a device is connected and has said what it is. */
    const info = $derived(session?.status === "ready" ? session.info : null);

    // The privacy rule, held here as well as in the card: the check is made when a device is
    // connected and never before. `ensure` is idempotent, so the two callers cost one request.
    $effect(() => {
        if (info) void firmwareCheck.ensure();
    });

    const offered = $derived(info ? firmwareCheck.offer(info.serialNumber, info.firmwareRevision) : null);
    // Never over a running transfer: the rider is watching a progress bar, and the header chip is
    // already saying what it is. The prompt returns when the cable is quiet — unless the transfer
    // was the update itself, which answers the question on its way out (`FirmwareCard`).
    const shown = $derived(jobRegistry.active === null ? offered : null);

    function answer(release: { version: string }) {
        firmwareCheck.answer(info?.serialNumber, release.version);
    }

    /**
     * Take the rider to the card.
     *
     * On a tier with a Device page that is a navigation, and the page arrives through a dynamic
     * import — so the element does not exist yet at the moment of the click. Hence the short poll
     * rather than one `scrollIntoView` into nothing; it gives up quietly after a second, by which
     * point either the page is there or something else is wrong.
     */
    function show(release: { version: string }) {
        answer(release);
        if (platform.caps.deviceDashboard && router.route !== "device") router.go("device");
        let tries = 0;
        const settle = () => {
            const card = document.getElementById(FIRMWARE_ANCHOR);
            if (card) {
                card.scrollIntoView({ behavior: "smooth", block: "center" });
            } else if (tries++ < 20) {
                setTimeout(settle, 50);
            }
        };
        settle();
    }
</script>

{#if shown}
    {@const release = shown}
    <div class="prompt card" role="status">
        <p class="line">Firmware {release.version} is available</p>
        {#if info}
            <p class="small muted">This device is running {info.firmwareRevision}.</p>
        {/if}
        <div class="actions">
            <button type="button" class="btn ghost" onclick={() => answer(release)}>Dismiss</button>
            <!-- A link on the tiers that have a Device page, so it behaves like one (middle-click,
                 the status bar) — the click still does the scrolling. -->
            {#if platform.caps.deviceDashboard}
                <a class="btn primary" href={DEVICE_ROUTE} onclick={() => show(release)}>View update</a>
            {:else}
                <button type="button" class="btn primary" onclick={() => show(release)}>View update</button>
            {/if}
        </div>
    </div>
{/if}

<style>
    /* Bottom-right, over the page and under the confirm dialog's backdrop (z-index 2000): a
       question the app is asking outranks a note it is offering. */
    .prompt {
        position: fixed;
        right: 16px;
        bottom: 16px;
        z-index: 1200;
        width: min(320px, calc(100% - 32px));
        display: flex;
        flex-direction: column;
        gap: 6px;
        box-shadow: 0 14px 34px rgba(32, 48, 29, 0.22);
    }

    .line {
        margin: 0;
        font-family: var(--serif);
        font-size: 15.5px;
    }

    .prompt p {
        margin: 0;
    }

    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        margin-top: 6px;
    }

    /* The link-shaped primary: `.btn` is written for buttons, so undo the anchor's own colour rule
       and keep the label from picking up an underline on hover. */
    a.btn.primary {
        color: var(--parchment);
        text-decoration: none;
    }
</style>
