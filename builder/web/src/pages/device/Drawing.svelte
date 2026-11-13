<script lang="ts">
    import { onMount } from 'svelte';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import { platform } from '../../lib/platform';
    import { openLiveSkinPreview, type LiveSkinPreview } from '../../lib/skin/preview';

    type Theme = 'day' | 'night';
    const themes: { value: Theme; label: string }[] = [{ value: 'day', label: 'Day' }, { value: 'night', label: 'Night' }];
    /** The panel of the device. */
    const WIDTH = 240, HEIGHT = 320;

    let canvas: HTMLCanvasElement;
    let theme = $state<Theme>('day');
    let preview: LiveSkinPreview | null = null;

    /** The skin preview draws 240 × 240. The rows below it come from a second frame with the camera moved down, so the
     * panel shows the map 1:1. `frame()` is a view of wasm memory: each one is copied before the next call. */
    function paint() {
        if (!preview || preview.width !== WIDTH) return;
        const rows = preview.height, rest = HEIGHT - rows, pixels = new Uint8ClampedArray(WIDTH * HEIGHT * 4);
        preview.setTheme(theme === 'night');
        pixels.set(preview.frame());
        preview.panBy(0, -rest);
        pixels.set(preview.frame().subarray((rows - rest) * WIDTH * 4), rows * WIDTH * 4);
        preview.resetCamera();
        canvas.getContext('2d')?.putImageData(new ImageData(pixels, WIDTH, HEIGHT), 0, 0);
    }

    function choose(next: Theme) {
        theme = next;
        paint();
    }

    onMount(() => {
        let live = true;
        // A sample map in the catalog skins, not the map of the rider. After an error the screen stays empty.
        void (async () => {
            const { body } = await platform.catalog();
            const skins: { id: string }[] = JSON.parse(body).skins;
            const skin = (id: string) => JSON.stringify(skins.find(entry => entry.id === id));
            const opened = await openLiveSkinPreview(body, skin('default'), skin('dusk'));
            if (!live) return opened.free();
            preview = opened;
            paint();
        })().catch(() => {});
        return () => {
            live = false;
            preview?.free();
            preview = null;
        };
    });
</script>

<div class="drawing">
    <div class="device">
        <div class="bezel"><canvas bind:this={canvas} width={WIDTH} height={HEIGHT} aria-label="The map screen of the device"></canvas></div>
        <span class="mark" aria-hidden="true">OBC</span>
        <span class="button left top"></span><span class="button left bottom"></span>
        <span class="button right top"></span><span class="button right bottom"></span>
    </div>
    <Segmented label="Day or night" options={themes} value={theme} onChange={choose} compact />
</div>

<style>
    .drawing {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 24px;
    }
    /* The housing as the iOS app draws it. Each size is a share of the body height, 470 units; the colours are those of
       the housing, the same in both looks of the page. */
    .device {
        --h: min(470px, 110vw);
        --u: calc(var(--h) / 470);
        --body: #2f6350;
        --rim: #8be3bc;
        --button: #2b2f36;
        --bezel: #101317;
        position: relative;
        flex: none;
        width: calc(308 * var(--u));
        height: var(--h);
        border-radius: calc(42 * var(--u));
        background: var(--body);
        box-shadow: 0 0 0 calc(6 * var(--u)) var(--rim), 0 calc(14 * var(--u)) calc(26 * var(--u)) rgb(47 99 80 / 0.3);
    }
    /* 240 + 2 × 16 wide, centred; the screen top is 32 below the body top. */
    .bezel {
        position: absolute;
        top: calc(16 * var(--u));
        left: calc(18 * var(--u));
        padding: calc(16 * var(--u));
        border-radius: calc(26 * var(--u));
        background: var(--bezel);
    }
    canvas {
        display: block;
        width: calc(240 * var(--u));
        height: calc(320 * var(--u));
        border-radius: calc(10 * var(--u));
        background: #fff;
        image-rendering: pixelated;
    }
    /* The letter spacing also follows the last letter, so the same space on the left keeps the word centred. */
    .mark {
        position: absolute;
        right: 0;
        bottom: calc(30 * var(--u));
        left: 0;
        padding-left: calc(308 * 0.05 * var(--u));
        color: rgb(139 227 188 / 0.28);
        font: 700 calc(308 * 0.13 * var(--u)) / 1 var(--mono);
        letter-spacing: calc(308 * 0.05 * var(--u));
        text-align: center;
    }
    /* Each pair of 66 + 22 + 66 is centred on the 470 of the body. */
    .button {
        position: absolute;
        width: calc(19 * var(--u));
        height: calc(66 * var(--u));
        border-radius: 2px;
        background: var(--button);
    }
    .left { left: calc(-13 * var(--u)); }
    .right { right: calc(-13 * var(--u)); }
    .top { top: calc(158 * var(--u)); }
    .bottom { top: calc(246 * var(--u)); }
</style>
