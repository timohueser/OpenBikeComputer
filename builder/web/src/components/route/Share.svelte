<script lang="ts">
    import Segmented from '../../lib/ui/Segmented.svelte';
    import Switch from '../../lib/ui/Switch.svelte';
    import Window from '../../lib/ui/Window.svelte';
    import { drawCard, FONTS, HIDDEN_M, SHORT_M, type Card, type Format } from '../../lib/covers/card';
    import { coverKey } from '../../lib/covers/job';
    import type { CoverPoints, RideSummary, RouteSummary } from '../../lib/library/records';
    import type { Library } from '../../lib/library/store';
    import { duration } from '../../lib/planner/editor';
    import { formatDistance, formatHeight, heightUnit } from '../../lib/settings/format';
    import { download, fileName } from './download';

    /** The window "Share": the share card as the file will be, and beside it the format, the switches of its parts
     * and the buttons that the browser can do: "Download", "Copy" and "Share". The window keeps the file of the card
     * that shows, so a click on a button waits for nothing. It remembers no format and no switch. */
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
    const FORMATS: { value: Format; label: string }[] = [{ value: 'wide', label: 'Wide' }, { value: 'square', label: 'Square' }, { value: 'tall', label: 'Tall' }];
    const canCopy = typeof ClipboardItem !== 'undefined' && !!navigator.clipboard?.write;
    const canShare = !!navigator.canShare?.({ files: [new File([], 'card.jpg', { type: 'image/jpeg' })] });

    interface Ground { bitmap: ImageBitmap; points: CoverPoints }
    let format = $state<Format>('wide');
    let picture = $state(true), line = $state(true), numbers = $state(true), profile = $state(true), hideEnds = $state(true), dated = $state(true);
    /** The cover of the line of today, and the picture of the map that the window makes when it opens with no cover. */
    let cover = $state.raw<Ground>(), map = $state.raw<Ground>();
    let canvas = $state<HTMLCanvasElement>(), file = $state.raw<Blob>();
    /** What the last "Copy" or "Share" did. */
    let said = $state('');

    const ground = $derived(cover ?? map);
    const ride = $derived('movingS' in summary);
    const heights = $derived(summary.heights.some(height => height !== null));
    const canHide = $derived(summary.lengthM >= SHORT_M);
    /** A formatted value has a space that does not break before its unit. */
    const split = (text: string): [string, string] => {
        const [amount, unit = ''] = text.split(' ');
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
        cover: ground && picture ? { image: ground.bitmap, width: ground.bitmap.width, height: ground.bitmap.height, points: ground.points, summary } : undefined,
        // With no picture, the window has no switch for the line.
        line: !ground || line,
        profile: heights && profile,
        trim: canHide && hideEnds ? HIDDEN_M : 0,
    });

    /** The text of the card, for a reader that does not see the canvas. */
    const label = $derived(`Share card: ${[card.date, name, ...card.numbers.map(number => number.join(' ').trim())].filter(Boolean).join(', ')}`);

    // The cover job of the page renders a missing cover; the card changes to the picture when it is there. A cover of
    // an earlier line is no picture of the card. With no cover, the window makes the picture of the map one time, in
    // the light look as the card, and does not store it; when that fails, the ground stays plain.
    $effect(() => {
        const target = id, abort = new AbortController();
        let live = true, asked = false;
        const mapGround = async () => {
            const { mapPicture } = await import('../../lib/covers/map-picture');
            const made = await mapPicture(summary, false, abort.signal, 'share').catch(() => 'none' as const);
            const bitmap = made === 'none' ? undefined : await createImageBitmap(made.image).catch(() => undefined);
            if (!live || made === 'none') return bitmap?.close();
            map = bitmap && { bitmap, points: made.points };
        };
        const load = async () => {
            const [item, stored] = await Promise.all([library.item(target), library.cover(target)]);
            const current = item && stored?.state === 'done' && stored.lineKey === coverKey(item) && stored.image && stored.points ? stored : undefined;
            const bitmap = current && await createImageBitmap(current.image!).catch(() => undefined);
            if (!live) return bitmap?.close();
            cover = bitmap && { bitmap, points: current!.points! };
            if (cover || asked) return;
            asked = true;
            void mapGround();
        };
        void load();
        const stop = library.onChange((store, key) => { if (key === target && store !== 'state') void load(); });
        return () => { live = false; abort.abort(); stop(); };
    });

    $effect(() => {
        const bitmap = cover?.bitmap;
        return () => bitmap?.close();
    });
    $effect(() => {
        const bitmap = map?.bitmap;
        return () => bitmap?.close();
    });

    $effect(() => {
        const [target, shape, content] = [canvas, format, card];
        if (!target) return;
        let live = true;
        [file, said] = [undefined, ''];
        const draw = () => {
            if (!live) return;
            drawCard(target, shape, content);
            target.toBlob(made => { if (live && made) file = made; }, 'image/jpeg', QUALITY);
        };
        // With its fonts loaded, as the page has them, the card has its size before the first paint. A font that does
        // not load, as with no network, leaves its fallback to the card.
        if (FONTS.every(font => document.fonts.check(font))) draw();
        else void Promise.allSettled(FONTS.map(font => document.fonts.load(font))).then(draw);
        return () => { live = false; };
    });

    // Safari permits the clipboard and the share sheet only in the click itself: no line here waits before its call.
    // The clipboard takes a PNG only. It gets the promise of one: a PNG of the card is too large to make after each
    // change as the JPEG file is.
    async function copy() {
        const png = new Promise<Blob>((resolve, reject) => canvas!.toBlob(made => made ? resolve(made) : reject(new Error('The card has no image.')), 'image/png'));
        try {
            await navigator.clipboard.write([new ClipboardItem({ 'image/png': png })]);
            said = 'Copied.';
        } catch {
            said = 'The browser did not copy the image.';
        }
    }

    async function share() {
        try {
            await navigator.share({ files: [new File([file!], fileName(name, 'jpg'), { type: 'image/jpeg' })] });
        } catch (reason) {
            // The rider closed the share sheet.
            if (!(reason instanceof DOMException && reason.name === 'AbortError')) said = 'The browser did not share the image.';
        }
    }
</script>

<Window title="Share" wide {onClose}>
    <div class="share">
        <!-- svelte-ignore a11y_no_interactive_element_to_noninteractive_role -->
        <canvas bind:this={canvas} role="img" aria-label={label}></canvas>
        <div class="side">
            <Segmented label="Format" options={FORMATS} value={format} onChange={next => format = next} />
            <div class="switches">
                {#if ground}
                    <div><Switch label="Picture" bind:checked={picture} /></div>
                    <div><Switch label="Line" bind:checked={line} /></div>
                {/if}
                {#if date !== undefined}<div><Switch label="Date" bind:checked={dated} /></div>{/if}
                <div><Switch label="Numbers" bind:checked={numbers} /></div>
                {#if heights}<div><Switch label="Elevation profile" bind:checked={profile} /></div>{/if}
                {#if canHide}<div><Switch label="Hide start and end" note="The first and last 500 m" bind:checked={hideEnds} /></div>{/if}
            </div>
            <p class="said" role="status">{said}</p>
            <div class="window-buttons">
                <button type="button" class="button primary" disabled={!file} onclick={() => file && download(file, 'image/jpeg', name, 'jpg')}>Download</button>
                {#if canCopy}<button type="button" class="button" disabled={!file} onclick={copy}>Copy</button>{/if}
                {#if canShare}<button type="button" class="button" disabled={!file} onclick={share}>Share</button>{/if}
            </div>
        </div>
    </div>
</Window>

<style>
    /* The side has one width and the card has one box, so no format and no switch moves a part of the window. */
    .share {
        display: grid;
        grid-template-columns: minmax(0, 1fr) 280px;
        gap: 24px;
    }
    /* The box of the card is a square: it holds the card of each format whole, at its top. In a low window the box is lower, so
       that the window does not scroll: the window is 48 px lower than the screen, and its header and its padding take
       80 px. */
    canvas {
        display: block;
        width: 100%;
        aspect-ratio: 1;
        max-height: calc(100dvh - 130px);
        object-fit: contain;
        object-position: top;
    }
    .side {
        display: flex;
        flex-direction: column;
    }
    .switches {
        margin-top: 14px;
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
    /* The line of the sentence is always there, so the sentence moves no button. */
    .said {
        box-sizing: content-box;
        min-height: 19px;
        margin: auto 0 0;
        padding-top: 12px;
        color: var(--ink-soft);
        font-size: 13.5px;
        text-align: right;
    }
    .side .window-buttons {
        margin-top: 8px;
    }
    /* The file of a changed card is ready in a moment: a button gets pale only when it takes longer. */
    .window-buttons button:disabled {
        transition: opacity 150ms 300ms;
    }
    @media (max-width: 760px) {
        .share {
            grid-template-columns: 1fr;
            gap: 16px;
        }
        /* The format, the switches and the buttons stay on the screen under the card. */
        canvas {
            max-height: 36dvh;
        }
    }
</style>
