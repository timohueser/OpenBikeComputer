import * as maplibregl from 'maplibre-gl';
import { placeCallout, TIP, type Box } from './callout-placement';
import type { Coordinate } from './map-types';

export type MapCallout = { show(at: Coordinate | null): void; refit(pan: boolean): void; remove(): void };

/**
 * A popup with `content` on `map`, anchored where it fits inside `free()`, the free box of the map. The content gets
 * the room of that box, less the offset and the tip, as `--callout-width` and `--callout-room`. A move of the map
 * anchors it again; a change of the size of the content, and the end of the move that `show` starts, also pan the map
 * the least distance when it fits nowhere. A pan never starts from a move, so one pan does not start another.
 */
export function mapCallout(map: maplibregl.Map, content: HTMLElement, { free, offset }: { free: () => Box; offset: number }): MapCallout {
    let popup: maplibregl.Popup | undefined, at: Coordinate | null = null;

    function refit(pan: boolean) {
        if (!popup || !at) return;
        const box = free(), { x, y } = map.project(at);
        content.style.setProperty('--callout-width', `${box.right - box.left}px`);
        content.style.setProperty('--callout-room', `${box.bottom - box.top - offset - TIP}px`);
        const placed = placeCallout([x, y], [content.offsetWidth, content.offsetHeight], box, offset);
        if (popup.options.anchor !== placed.anchor) {
            popup.options.anchor = placed.anchor;
            popup.setOffset(offset);
        }
        const duration = matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 400;
        // The planner keeps its search through a move of this pan.
        if (pan && (placed.pan[0] || placed.pan[1])) map.panBy(placed.pan, { duration }, { preserveSearch: true });
    }

    const move = () => refit(false);
    const observer = new ResizeObserver(() => requestAnimationFrame(() => refit(true)));
    map.on('move', move);
    observer.observe(content);

    return {
        show(next) {
            at = next;
            if (!at) {
                popup?.remove();
                popup = undefined;
                return;
            }
            popup ??= new maplibregl.Popup({ closeButton: false, closeOnClick: false, offset, maxWidth: '340px', anchor: 'bottom', className: 'map-callout' })
                .setDOMContent(content);
            popup.setLngLat(at).addTo(map);
            refit(false);
            const settle = () => requestAnimationFrame(() => refit(true));
            if (map.isMoving()) map.once('moveend', settle);
            else settle();
        },
        refit,
        remove() {
            map.off('move', move);
            observer.disconnect();
            popup?.remove();
        },
    };
}
