<script lang="ts">
    import { onMount } from 'svelte';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import { platform } from '../../lib/platform';
    import { openLiveSkinPreview, type LiveSkinPreview } from '../../lib/skin/preview';

    type Theme = 'day' | 'night';
    const themes: { value: Theme; label: string }[] = [{ value: 'day', label: 'Day' }, { value: 'night', label: 'Night' }];

    let canvas: HTMLCanvasElement;
    let theme = $state<Theme>('day');
    let preview: LiveSkinPreview | null = null;

    function paint() {
        if (!preview) return;
        preview.setTheme(theme === 'night');
        canvas.getContext('2d')?.putImageData(new ImageData(new Uint8ClampedArray(preview.frame()), preview.width, preview.height), 0, 0);
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
        <canvas bind:this={canvas} width="240" height="240" aria-label="The map screen of the device"></canvas>
        <div class="buttons"><span></span><span></span><span></span></div>
    </div>
    <Segmented label="Day or night" options={themes} value={theme} onChange={choose} compact />
</div>

<style>
    .drawing {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 18px;
    }
    /* The device is black in both looks of the page. */
    .device {
        padding: 26px 24px 0;
        border-radius: 34px;
        background: #1c1b14;
        box-shadow: 0 12px 30px rgb(28 27 20 / 0.25);
    }
    canvas {
        display: block;
        width: min(260px, 60vw);
        aspect-ratio: 1;
        border-radius: 3px;
        background: #3a3930;
        image-rendering: pixelated;
    }
    .buttons {
        display: flex;
        justify-content: center;
        gap: 44px;
        padding: 22px 0 24px;
    }
    .buttons span {
        width: 16px;
        height: 16px;
        border-radius: 50%;
        background: #3f3e36;
    }
</style>
