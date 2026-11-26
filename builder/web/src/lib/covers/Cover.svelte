<script lang="ts">
    import { untrack } from 'svelte';
    import type { CoverPoints, CoverRecord, RideRecord, RouteRecord, Summary } from '../library/records';
    import type { Library } from '../library/store';
    import { appearance } from '../theme.svelte';
    import { lineKey } from '../library/summary';
    import { coverKey, settled, wantCover, wantsPicture, type Place } from './job';
    import CoverPicture from './CoverPicture.svelte';

    let { library, id, mark, part, line = true, place = 'row', pins = [], photos = [], onphoto, onpoint, credits = $bindable([]) }: {
        library: Library;
        /** The id of a route or the key of a ride. */
        id: string;
        /** Metres along the route, as `along` of line.ts reads them. */
        mark?: number;
        part?: [number, number];
        line?: boolean;
        /** A row and a card show the photo that the rider chose for a ride, else the map picture when the record has no
         * cover. A row and a page ask the cover job for the cover, a card also for the map picture. */
        place?: Place;
        /** Labels at shares of the summary line from 0 to 1: a numbered waypoint, or a night without a number. */
        pins?: { share: number; label: string; number?: number }[];
        /** The photos of a ride at shares of the summary line, each with the address of its small picture. */
        photos?: { id: string; url: string; share: number }[];
        onphoto?: (id: string) => void;
        /** The metres along the route of the point nearest to the pointer, or null when the pointer leaves. */
        onpoint?: (metres: number | null) => void;
        /** The credits of the data in the shown image: the cover or the map picture. */
        credits?: string[];
    } = $props();

    /** The image that the part shows, with the points of its line, and the summary that the points belong to. */
    interface Shown { bitmap: ImageBitmap; points: CoverPoints; summary?: Summary }
    let item = $state<RouteRecord | RideRecord>(), shown = $state<Shown>();
    /** The address of the copy of the chosen photo of a ride. */
    let photo = $state<string>();
    /** The theme of the map picture that a record with no cover image shows; a cover does not depend on the theme. */
    let pictureDark = $state<boolean>();
    let reload: (() => void) | undefined;
    const look = $derived(item && 'key' in item ? 'ride' : 'route');

    /** The cover, else on a row or a card the map picture of the theme for the line. */
    function source(item: RouteRecord | RideRecord, cover: CoverRecord, dark: boolean) {
        const current = cover.lineKey === coverKey(item), summary = item.summary ?? undefined;
        // The points of a cover of an earlier line draw no mark and no part.
        if (cover.image && cover.points) return { image: cover.image, points: cover.points, summary: current && cover.state === 'done' ? summary : undefined, credits: cover.credits };
        const map = place !== 'page' && cover.map?.lineKey === lineKey(item) ? cover.map : undefined, image = map?.[dark ? 'dark' : 'light'];
        return image && map?.points ? { image, points: map.points, summary, credits: map.credits } : undefined;
    }

    $effect(() => {
        const target = id;
        let live = true;
        const load = async (first: boolean) => {
            const dark = place !== 'page' && untrack(() => appearance.dark);
            const [nextItem, nextCover] = await Promise.all([library.item(target), library.cover(target)]);
            const chosen = place !== 'page' && nextItem && 'key' in nextItem && nextItem.photo ? await library.photo(nextItem.photo) : undefined;
            const from = chosen ? undefined : nextItem && nextCover && source(nextItem, nextCover, dark);
            const bitmap = from ? await createImageBitmap(from.image).catch(() => undefined) : undefined;
            if (!live) return bitmap?.close();
            [item, shown, photo] = [nextItem, from && bitmap && { bitmap, points: from.points, summary: from.summary }, chosen && URL.createObjectURL(chosen.image)];
            credits = shown ? from?.credits ?? [] : [];
            pictureDark = place !== 'page' && !chosen && !nextCover?.image ? dark : undefined;
            if (!first || !nextItem?.summary) return;
            if (!settled(nextItem, nextCover) || (place === 'card' && wantsPicture(nextItem, nextCover, dark))) wantCover(target, place);
        };
        reload = () => void load(true);
        void load(true);
        const stop = library.onChange((store, key) => { if (key === target && store !== 'state') void load(false); });
        return () => { live = false; stop(); };
    });

    // A map picture loads again after a change of the theme, and asks for the picture of the new theme.
    $effect(() => {
        if (pictureDark !== undefined && appearance.dark !== pictureDark) untrack(() => reload?.());
    });

    $effect(() => {
        const bitmap = shown?.bitmap;
        return () => bitmap?.close();
    });

    $effect(() => {
        const url = photo;
        return () => { if (url) URL.revokeObjectURL(url); };
    });
</script>

<!-- The pages of a route and a ride show their own pins at the start. -->
{#if photo}
    <img class="card-photo" src={photo} alt="" />
{:else}
    <CoverPicture {shown} summary={item?.summary ?? undefined} {look} {mark} {part} {line} start={place !== 'page'} {pins} {photos} {onphoto} {onpoint} />
{/if}

<style>
    .card-photo {
        display: block;
        width: 100%;
        height: 100%;
        object-fit: cover;
    }
</style>
