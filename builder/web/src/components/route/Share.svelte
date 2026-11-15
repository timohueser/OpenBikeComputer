<script lang="ts">
    import Switch from '../../lib/ui/Switch.svelte';
    import Window from '../../lib/ui/Window.svelte';
    import { drawCard, FONTS, HIDDEN_M, layout, SHORT_M, type Card, type Format } from '../../lib/covers/card';
    import { coverKey } from '../../lib/covers/job';
    import type { CoverPoints, RideSummary, RouteSummary } from '../../lib/library/records';
    import type { Library } from '../../lib/library/store';
    import { duration } from '../../lib/planner/editor';
    import { formatDistance, formatHeight, heightUnit } from '../../lib/settings/format';
    import { download } from './download';

    /** The window "Share": the share card as the file will be, the switches of its parts beside it, and "Download".
     * The window keeps the file of the card that shows, so a click on a button waits for nothing. It remembers no switch. */
    let { library, id, name, summary, date, onClose }: {
        library: Library;
        /** The id of a route or the key of a ride. */
        id: string;
        name: string;
        summary: RouteSummary | RideSummary;
        /** The date of a ride, as its page shows it. */
        date?: string;
        onClose: () => void;
    } = $props();

    const QUALITY = 0.9;
    const format: Format = 'wide';
    let picture = $state(true), line = $state(true), numbers = $state(true), profile = $state(true), hideEnds = $state(true), dated = $state(true);
    /** The size of the canvas before the first drawing: the card of a name of one line. */
    const [width, height] = layout(format, { nameLines: 1, date: false, numbers: true, profile: true, credit: false }).size;
    /** The cover of the line of today. */
    let cover = $state.raw<{ bitmap: ImageBitmap; points: CoverPoints }>();
    let canvas = $state<HTMLCanvasElement>(), file = $state.raw<Blob>();

    const ride = $derived('movingS' in summary);
    const heights = $derived(summary.heights.some(height => height !== null));
    const canHide = $derived(summary.lengthM >= SHORT_M);
    /** A formatted value has a space that does not break before its unit. */
    const split = (text: string): [string, string] => {
        const [amount, unit = ''] = text.split('\u00a0');
        return [amount, unit];
    };
    const card = $derived<Card>({
        name, summary, look: ride ? 'ride' : 'route',
        date: dated ? date : undefined,
        numbers: numbers ? [
            split(formatDistance(summary.lengthM)),
            [split(formatHeight(summary.climbM))[0], `${heightUnit()} up`],
            [duration(('movingS' in summary ? summary.movingS : summary.seconds) / 3600), ''],
        ] : [],
        cover: cover && picture ? { image: cover.bitmap, width: cover.bitmap.width, height: cover.bitmap.height, points: cover.points, summary } : undefined,
        // With no cover, the window has no switch for the line.
        line: !cover || line,
        profile: heights && profile,
        trim: canHide && hideEnds ? HIDDEN_M : 0,
    });

    /** The text of the card, for a reader that does not see the canvas. */
    const label = $derived(`Share card: ${[card.date, name, ...card.numbers.map(number => number.join(' ').trim())].filter(Boolean).join(', ')}`);

    // The cover job of the page renders a missing cover; the card changes to the picture when it is there. A cover of
    // an earlier line is no picture of the card.
    $effect(() => {
        const target = id;
        let live = true;
        const load = async () => {
            const [item, stored] = await Promise.all([library.item(target), library.cover(target)]);
            const current = item && stored?.state === 'done' && stored.lineKey === coverKey(item) && stored.image && stored.points ? stored : undefined;
            const bitmap = current && await createImageBitmap(current.image!).catch(() => undefined);
            if (!live) return bitmap?.close();
            cover = bitmap && { bitmap, points: current!.points! };
        };
        void load();
        const stop = library.onChange((store, key) => { if (key === target && store !== 'state') void load(); });
        return () => { live = false; stop(); };
    });

    $effect(() => {
        const bitmap = cover?.bitmap;
        return () => bitmap?.close();
    });

    $effect(() => {
        const [target, content] = [canvas, card];
        if (!target) return;
        let live = true;
        file = undefined;
        // A font that does not load, as with no network, leaves its fallback to the card.
        void Promise.allSettled(FONTS.map(font => document.fonts.load(font))).then(() => {
            if (!live) return;
            drawCard(target, format, content);
            target.toBlob(made => { if (live && made) file = made; }, 'image/jpeg', QUALITY);
        });
        return () => { live = false; };
    });
</script>

<Window title="Share" wide {onClose}>
    <div class="share">
        <!-- svelte-ignore a11y_no_interactive_element_to_noninteractive_role -->
        <canvas bind:this={canvas} {width} {height} role="img" aria-label={label}></canvas>
        <div class="side">
            <div class="switches">
                {#if cover}
                    <div><Switch label="Picture" bind:checked={picture} /></div>
                    <div><Switch label="Line" bind:checked={line} /></div>
                {/if}
                {#if date !== undefined}<div><Switch label="Date" bind:checked={dated} /></div>{/if}
                <div><Switch label="Numbers" bind:checked={numbers} /></div>
                {#if heights}<div><Switch label="Elevation profile" bind:checked={profile} /></div>{/if}
                {#if canHide}<div><Switch label="Hide start and end" note="The first and last 500 m" bind:checked={hideEnds} /></div>{/if}
            </div>
            <div class="window-buttons">
                <button type="button" class="button primary" disabled={!file} onclick={() => file && download(file, 'image/jpeg', name, 'jpg')}>Download</button>
            </div>
        </div>
    </div>
</Window>

<style>
    /* The side has one width, and no switch changes the size of the card: no switch moves a part of the window. */
    .share {
        display: grid;
        grid-template-columns: minmax(0, 1fr) 280px;
        gap: 24px;
    }
    /* The card keeps its shape. In a low window it gets smaller, so that the window does not scroll: the window is
       48 px lower than the screen, and its header and its padding take 80 px. */
    canvas {
        display: block;
        max-width: 100%;
        max-height: calc(100dvh - 130px);
        border-radius: 8px;
        background: var(--parchment-2);
    }
    .side {
        display: flex;
        flex-direction: column;
    }
    .switches {
        font-weight: 700;
    }
    .switches > div {
        padding: 8px 0;
        border-top: 1px solid var(--line);
    }
    .switches > div:first-child {
        padding-top: 0;
        border-top: 0;
    }
    .side .window-buttons {
        margin-top: auto;
        padding-top: 20px;
    }
    /* The file of a changed card is ready in a moment: the button gets pale only when it takes longer. */
    .window-buttons button:disabled {
        transition: opacity 150ms 300ms;
    }
    @media (max-width: 760px) {
        .share {
            grid-template-columns: 1fr;
            gap: 16px;
        }
        /* The switches and the button stay on the screen under the card. */
        canvas {
            max-height: 40dvh;
            margin: 0 auto;
        }
    }
</style>
