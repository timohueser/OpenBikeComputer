<script lang="ts">
    import Header from "./components/Header.svelte";
    import Desktop from "./routes/Desktop.svelte";
    import Home from "./routes/Home.svelte";
    import { loadStyleEditor, type StyleEditorModule } from "./lib/platform";
    import { available, DESKTOP_ADDS } from "./lib/platform/gating";
    import { router } from "./lib/router.svelte";

    // The advanced editor is desktop-only (the locked decision in #894), so it
    // is reached through an `import()` that only the hosts with
    // `caps.styleEditor` declare — the web host has no reference to the route
    // anywhere in its graph, so Rollup can't emit the chunk at all rather than
    // emitting one nothing loads. Copied to a local const so the narrowing
    // survives into the closure; the promise is memoized so re-renders don't
    // restart the await block.
    const load = loadStyleEditor;
    let editor: Promise<StyleEditorModule> | undefined;
    const openEditor = load ? () => (editor ??= load()) : null;

    const showEditor = $derived(router.route === "advanced" && openEditor !== null);
    // Same shape as the editor's gate: a host that is missing nothing has
    // nothing to read there, so #/desktop falls back to home rather than
    // rendering a page that pitches the app you are already running.
    const showDesktop = $derived(router.route === "desktop" && DESKTOP_ADDS.length > 0);
    const showHome = $derived(!showEditor && !showDesktop);
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
    <div class="route" hidden={!showHome}>
        <Home active={showHome} />
    </div>
    {#if showEditor && openEditor}
        {#await openEditor() then { default: StyleEditor }}
            <StyleEditor />
        {/await}
    {/if}
    {#if showDesktop}
        <Desktop />
    {/if}
</main>

<footer class="faint small">
    Map data © <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors ·
    extracts by <a href="https://download.geofabrik.de/">Geofabrik</a>{#if available("build")}{" · builds run locally on this machine"}{/if}
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
        width: min(1400px, 100% - 32px);
        margin: 0 auto;
        padding: 18px 0 28px;
        display: flex;
        flex-direction: column;
    }

    /* Fill the viewport so the map (Home's grid) grows into tall screens. */
    .route {
        flex: 1;
        display: flex;
        flex-direction: column;
    }

    .route[hidden] {
        display: none;
    }

    footer {
        width: min(1400px, 100% - 32px);
        margin: 0 auto;
        padding: 10px 0 18px;
        border-top: 1px solid var(--line);
    }
</style>
