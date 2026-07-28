<!-- The device page: what is on the card (routes & trips, rides, firmware).
     Placeholder — the thumbdrive view lands in this file next. -->
<script lang="ts">
    import { deviceHolder } from "../lib/device/session.svelte";

    const session = $derived(deviceHolder.session);
</script>

<article>
    {#if session?.status === "ready"}
        <p class="muted">Connected.</p>
    {:else}
        <section class="card empty">
            <p class="big">No device connected</p>
            <p class="small muted">Plug the OpenBikeComputer in over USB — it will appear here by itself.</p>
        </section>
    {/if}
</article>

<style>
    article {
        width: min(860px, 100%);
        margin: 0 auto;
    }

    .empty {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 8px;
        padding: 48px 24px;
        text-align: center;
    }

    .big {
        font-family: var(--serif);
        font-size: 18px;
        margin: 0;
    }

    .empty p {
        margin: 0;
    }
</style>
