<!--
  The device step: connect a plugged-in OBC, then write to it (C4, #903).

  Gated `["deviceUsb", "webUsb"]` — tier first, browser second — so a visitor on a tier without USB
  and a visitor on Safari get different sentences, both of them written once in `GATES`.

  The connect button is not a nicety. WebUSB's chooser may only open from a real click, so a page
  that auto-detects has to *also* have a button for the first visit; `requestDevice()` is called
  straight out of the click handler, with no await in front of it, or the browser refuses.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import Gated from "../Gated.svelte";
    import { platform } from "../../lib/platform";
    import { deviceHolder } from "../../lib/device/session.svelte";
    import type { MapArtifact } from "../../lib/device/write";

    let { artifact = null }: { artifact?: MapArtifact | null } = $props();

    // The write surfaces reach the protocol client, the codecs and the transport — the ~24 kB C3
    // code-split out of the entry bundle. Loading them on connect keeps that split: a visitor who
    // only downloads a map never fetches the USB stack. Memoized so a re-render does not restart
    // the fetch, exactly as `App.svelte` does for the style editor.
    let surfaces: Promise<typeof import("./DeviceSurfaces.svelte")> | undefined;
    const loadSurfaces = () => (surfaces ??= import("./DeviceSurfaces.svelte"));

    // Opening adopts an already-permitted device with no prompt; it is not connecting, and on a
    // first visit it finds nothing, which is the ordinary outcome rather than an error.
    onMount(() => void deviceHolder.open());

    const session = $derived(deviceHolder.session);
    let prompting = $state(false);

    function connect() {
        const current = session;
        if (!current) return;
        prompting = true;
        // The account of the last interrupted write has been read by now — the rider is acting on
        // it. Clearing it here rather than on `ready` keeps it up while the reconnect runs.
        deviceHolder.interrupted = null;
        // No await before this call: the browser checks that the chooser was opened from the
        // click's own call stack.
        void current.requestDevice().finally(() => (prompting = false));
    }
</script>

<Gated need={["deviceUsb", "webUsb"]} value={platform.device}>
    {#snippet children()}
        {#if deviceHolder.error}
            <p class="note error small" role="alert">{deviceHolder.error}</p>
        {:else if !session}
            <p class="small muted">Looking for a device…</p>
        {:else if session.status === "unsupported"}
            <p class="small muted">{session.error}</p>
        {:else if session.status === "ready" && session.client}
            <div class="head">
                <p class="small">
                    <span class="dot" aria-hidden="true"></span>
                    Connected
                    {#if session.info}<span class="faint mono">{session.info.hardwareRevision}</span>{/if}
                </p>
                <button type="button" class="btn ghost" onclick={() => void session.disconnect()}>
                    Disconnect
                </button>
            </div>

            {#await loadSurfaces()}
                <p class="small muted">Loading…</p>
            {:then { default: Surfaces }}
                <Surfaces
                    client={session.client}
                    info={session.info}
                    identity={session.identity}
                    {artifact}
                />
            {:catch}
                <p class="note error small" role="alert">
                    The device tools could not be loaded. Check your connection and reload the page.
                </p>
            {/await}
        {:else if session.status === "connecting"}
            <p class="small muted">Connecting…</p>
        {:else}
            {#if deviceHolder.interrupted}
                <p class="note error small" role="alert">{deviceHolder.interrupted}</p>
            {/if}
            <div class="connect">
                <button type="button" class="btn primary" disabled={prompting} onclick={connect}>
                    Connect device
                </button>
                <p class="small faint">Plug the OBC in over USB, then pick it from the browser's list.</p>
            </div>
            {#if session.status === "error" && session.error}
                <p class="note error small" role="alert">{session.error}</p>
            {/if}
        {/if}
    {/snippet}

    {#snippet unavailable(reason)}
        <button type="button" class="btn primary" disabled aria-describedby={reason}>Connect device</button>
    {/snippet}
</Gated>

<style>
    .head {
        display: flex;
        align-items: center;
        gap: 10px;
        padding-bottom: 10px;
        margin-bottom: 4px;
        border-bottom: 1px solid var(--line);
    }

    .head p {
        margin: 0;
        display: flex;
        align-items: baseline;
        gap: 8px;
    }

    .head .btn {
        margin-left: auto;
    }

    .dot {
        width: 7px;
        height: 7px;
        border-radius: 50%;
        background: var(--forest);
        display: inline-block;
    }

    .connect {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
    }

    .connect p {
        margin: 0;
    }

    .note {
        margin: 8px 0 0;
    }

    .error {
        color: var(--coral);
    }

    :global(.block + .block) {
        margin-top: 16px;
        padding-top: 16px;
        border-top: 1px solid var(--line);
    }
</style>
