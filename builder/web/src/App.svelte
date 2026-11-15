<script lang="ts">
    import ConfirmDialog from "./components/ConfirmDialog.svelte";
    import Header from "./components/Header.svelte";
    import UpdatePrompt from "./components/UpdatePrompt.svelte";
    import Home from "./routes/Home.svelte";
    import { LINKS } from "./lib/constants";
    import { platform, type StyleEditorModule } from "./lib/platform";
    import { router } from "./lib/router.svelte";

    // The advanced editor is maintainer tooling, reached through an `import()`
    // only the dev host declares — product hosts have no reference to the route
    // anywhere in its graph, so Rollup can't emit the chunk at all rather than
    // emitting one nothing loads. Copied to a local const so the narrowing
    // survives into the closure; the promise is memoized so re-renders don't
    // restart the await block.
    const load = platform.styleEditor?.load;
    let editor: Promise<StyleEditorModule> | undefined;
    const openEditor = load ? () => (editor ??= load()) : null;

    const showEditor = $derived(router.route === "advanced" && openEditor !== null);
    const showHome = $derived(!showEditor);
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
</main>

<footer class="faint small">
    <span>Map data © <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors</span>
    <span class="legal">
        <a href={LINKS.licenses}>Licences</a>
        <a href={LINKS.impressum}>Impressum</a>
        <a href={LINKS.datenschutz}>Datenschutz</a>
    </span>
</footer>

<!-- Mounted once, at the root: the app's own "are you sure?" (lib/ui/confirm.svelte.ts). -->
<ConfirmDialog />

<!-- Also mounted once, for the same reason: a device can be plugged in from any page, and the
     "there is newer firmware" note belongs to the device rather than to whichever page is open. -->
<UpdatePrompt />

<style>
    footer {
        display: flex;
        flex-wrap: wrap;
        justify-content: space-between;
        gap: 6px 16px;
    }

    footer .legal {
        display: flex;
        gap: 14px;
    }

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
        min-height: 0; /* the classic flex-overflow unlock: without it, children grow main past the viewport */
        width: min(1400px, 100% - 32px);
        margin: 0 auto;
        padding: 18px 0 28px;
        display: flex;
        flex-direction: column;
        overflow-y: auto; /* every ordinary route scrolls here… */
    }

    /* …while Home clamps to the viewport: the map pane takes exactly the room
       the window gives it, and Home's own steps column is what scrolls. */
    .route {
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
        overflow: hidden;
    }

    .route[hidden] {
        display: none;
    }

    @media (max-width: 940px) {
        main,
        .route {
            overflow: visible;
        }
    }

    footer {
        width: min(1400px, 100% - 32px);
        margin: 0 auto;
        padding: 10px 0 18px;
        border-top: 1px solid var(--line);
    }
</style>
