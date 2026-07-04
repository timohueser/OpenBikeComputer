<script lang="ts">
    import Header from "./components/Header.svelte";
    import Advanced from "./routes/Advanced.svelte";
    import Home from "./routes/Home.svelte";
    import { router } from "./lib/router.svelte";
</script>

<!-- Contour-line backdrop, the field-guide signature (see docs/index.html). -->
<svg class="backdrop" viewBox="0 0 1200 800" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
    <path d="M-40 160 C 200 80, 420 220, 660 140 S 1080 180, 1260 120" />
    <path d="M-40 320 C 240 240, 460 380, 700 300 S 1100 340, 1260 280" />
    <path d="M-40 500 C 220 420, 480 560, 720 480 S 1120 520, 1260 460" />
    <path d="M-40 680 C 260 600, 500 720, 740 640 S 1140 700, 1260 620" />
</svg>

<Header />

<!-- Home stays mounted (display toggle) so the Leaflet map survives navigation. -->
<main>
    <div class="route" hidden={router.route !== "home"}>
        <Home active={router.route === "home"} />
    </div>
    {#if router.route === "advanced"}
        <Advanced />
    {/if}
</main>

<footer class="faint small">
    Map data © <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors ·
    extracts by <a href="https://download.geofabrik.de/">Geofabrik</a> · builds run locally on this
    machine
</footer>

<style>
    .backdrop {
        position: fixed;
        inset: 0;
        width: 100%;
        height: 100%;
        z-index: -1;
        pointer-events: none;
    }

    .backdrop path {
        fill: none;
        stroke: var(--wood);
        stroke-opacity: 0.14;
        stroke-width: 1.4;
    }

    main {
        flex: 1;
        width: min(1180px, 100% - 32px);
        margin: 0 auto;
        padding: 18px 0 28px;
    }

    .route[hidden] {
        display: none;
    }

    footer {
        width: min(1180px, 100% - 32px);
        margin: 0 auto;
        padding: 10px 0 18px;
        border-top: 1px solid var(--line);
    }
</style>
