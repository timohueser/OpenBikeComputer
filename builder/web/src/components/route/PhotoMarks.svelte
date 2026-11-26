<script lang="ts">
    /** The photos of a ride on a picture or on a map: a small photo at each place. Photos closer than NEAR_PX are one
     * mark with a count, which shows the first of them. A click opens the photo of the mark. */
    let { photos, onopen }: {
        /** In the order of the ride, with their places in pixels of the box. */
        photos: { id: string; url: string; x: number; y: number }[];
        onopen?: (id: string) => void;
    } = $props();

    const NEAR_PX = 40;
    const marks = $derived.by(() => {
        const found: { id: string; url: string; x: number; y: number; count: number }[] = [];
        for (const photo of photos) {
            const near = found.find(mark => Math.hypot(mark.x - photo.x, mark.y - photo.y) < NEAR_PX);
            if (near) near.count++;
            else found.push({ ...photo, count: 1 });
        }
        return found;
    });
</script>

{#each marks as mark (mark.id)}
    <button type="button" class="photo-mark" style:left={`${mark.x}px`} style:top={`${mark.y}px`}
        aria-label={mark.count > 1 ? `${mark.count} photos` : 'Photo'} onclick={() => onopen?.(mark.id)}>
        <img src={mark.url} alt="" draggable="false" />
        {#if mark.count > 1}<b>{mark.count}</b>{/if}
    </button>
{/each}

<style>
    /* The white edge and the shadow of a pin, in both looks: the mark stands on a picture, not on the panel. */
    .photo-mark {
        position: absolute;
        width: 30px;
        height: 30px;
        padding: 0;
        border: 2px solid #fff;
        border-radius: 7px;
        background: #fff;
        box-shadow: 0 1px 4px rgb(28 27 20 / 0.35);
        transform: translate(-50%, -50%);
        pointer-events: auto;
    }
    .photo-mark:hover, .photo-mark:focus-visible {
        z-index: 1;
    }
    .photo-mark:focus-visible {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
    }
    img {
        display: block;
        width: 100%;
        height: 100%;
        border-radius: 5px;
        object-fit: cover;
    }
    b {
        position: absolute;
        top: -8px;
        right: -8px;
        box-sizing: border-box;
        min-width: 17px;
        height: 17px;
        padding: 0 4px;
        border-radius: 9px;
        background: rgb(28 27 20);
        color: #fff;
        font: 700 10.5px/17px var(--sans);
        font-variant-numeric: tabular-nums;
    }
</style>
