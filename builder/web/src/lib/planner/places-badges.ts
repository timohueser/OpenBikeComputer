// PROTOTYPE (never merges): the badges of ?places=badges as HTML marks, the list of a badge and the two motions.
//
//   ?badge=tile|stack|ring|pill   the shape of a badge (default tile; pill is the first prototype)
//   ?tilemax=8                    tile: the most cells. 4 = a strict 2 x 2 grid: three icons and "+N"
//   ?slow=5                       the motions run this many times slower (for the videos)
//   ?quiet=glyph|dot|none         glyph: the bare icon from minzoom; dot: a 6 px dot from minzoom, the bare icon from 16;
//                                 none: the bare icon from 16. Places that would cover each other are one 8 px ring.
//   ?halo ?onedot                 quiet: a halo in the land colour round the bare icon; one colour for all dots
//   ?highlight=a ?dim=1           kind a: a rust disc from zoom 12, a second disc behind it for several places
import './places-badges.css';
import type maplibregl from 'maplibre-gl';
import type { GeoJSONSource } from 'maplibre-gl';
import { categoryIds, placeCategories, poiKinds, type PlaceCategory } from './poi-kinds';

export type PlaceFeature = GeoJSON.Feature<GeoJSON.Point, { kind: string; category: PlaceCategory; name: string; lon: number; lat: number }>;
export type Shape = 'pill' | 'tile' | 'stack' | 'ring';
export type Quiet = 'glyph' | 'dot' | 'none';
/** The look of one mark in the quiet modes. */
type Look = 'glyph' | 'dot' | 'hl';
/** Quiet: places nearer than this many px are one mark. */
const RADIUS: Record<Look, number> = { glyph: 18, dot: 9, hl: 26 };
const HL_ZOOM = 12, BARE = 16;

const query = new URLSearchParams(typeof location === 'undefined' ? '' : location.search);
const number = (key: string, fallback: number) => (query.has(key) && Number.isFinite(Number(query.get(key))) ? Number(query.get(key)) : fallback);
const SHAPES: Shape[] = ['tile', 'stack', 'ring', 'pill'];
export const badgeShape: Shape = SHAPES.find(shape => shape === query.get('badge')) ?? 'tile';
const TILE_MAX = number('tilemax', 8), SLOW = number('slow', 1);

/** The order of the kinds in a badge and in its list: how often a bikepacker needs the kind in a day. */
const FIRST: PlaceCategory[] = ['water', 'shop', 'food', 'camp', 'bike', 'station', 'peak'];
export const ORDER: PlaceCategory[] = [...FIRST, ...categoryIds.filter(id => !FIRST.includes(id))];
const rank = (category: PlaceCategory) => ORDER.indexOf(category);

const EASE_OUT = 'cubic-bezier(0.23, 1, 0.32, 1)';
const LIST_IN = 160, LIST_OUT = 110, SPLIT_MS = 180, FADE_MS = 100;
const still = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

// ---- The groups of each zoom: places nearer than `radius` px are one node. A zoom step in only separates nodes.

type Node = { x: number; y: number; places: PlaceFeature[]; key?: string };
const WORLD = 512;
const worldX = (lon: number) => (lon + 180) / 360 * WORLD;
const worldY = (lat: number) => (1 - Math.log(Math.tan(lat * Math.PI / 180) + 1 / Math.cos(lat * Math.PI / 180)) / Math.PI) / 2 * WORLD;
const lonOf = (x: number) => x / WORLD * 360 - 180;
const latOf = (y: number) => Math.atan(Math.sinh(Math.PI * (1 - 2 * y / WORLD))) * 180 / Math.PI;

function levels(features: PlaceFeature[], radius: number, from: number, to: number): Map<number, Node[]> {
    let nodes: Node[] = features.map(feature => ({ x: worldX(feature.properties.lon), y: worldY(feature.properties.lat), places: [feature] }));
    const out = new Map<number, Node[]>([[to + 1, nodes]]);
    for (let zoom = to; zoom >= from; zoom--) {
        const reach = radius / 2 ** zoom, cells = new Map<string, number[]>(), current = nodes;
        const cell = (node: Node, dx = 0, dy = 0) => `${Math.floor(node.x / reach) + dx}/${Math.floor(node.y / reach) + dy}`;
        current.forEach((node, index) => { const list = cells.get(cell(node)); if (list) list.push(index); else cells.set(cell(node), [index]); });
        const used = new Uint8Array(current.length), next: Node[] = [];
        current.forEach((node, index) => {
            if (used[index]) return;
            used[index] = 1;
            const members = [node];
            for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++) for (const other of cells.get(cell(node, dx, dy)) ?? []) {
                if (used[other] || Math.hypot(current[other].x - node.x, current[other].y - node.y) > reach) continue;
                used[other] = 1;
                members.push(current[other]);
            }
            if (members.length === 1) { next.push(node); return; }
            const places = members.flatMap(member => member.places);
            next.push({
                x: members.reduce((sum, member) => sum + member.x * member.places.length, 0) / places.length,
                y: members.reduce((sum, member) => sum + member.y * member.places.length, 0) / places.length,
                places,
            });
        });
        out.set(zoom, nodes = next);
    }
    return out;
}

/** The same places make the same key, so a mark that a zoom step or new data does not change keeps its element. */
function keyOf(node: Node): string {
    if (node.key) return node.key;
    const ids = node.places.map(place => place.id as number);
    return node.key = ids.length === 1 ? `p${ids[0]}` : `${ids.length}:${Math.min(...ids)}:${Math.max(...ids)}:${ids.reduce((sum, id) => sum + id, 0)}`;
}

// ---- The look of a mark.

const glyph = (category: PlaceCategory, size = 12) =>
    `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="${(1.1 * 24 / size).toFixed(2)}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${placeCategories[category].icon}"/></svg>`;

/** A quiet mark or a highlighted disc. */
function quietBody(look: Look, kinds: PlaceCategory[], count: number): HTMLElement {
    const body = document.createElement('span');
    if (look === 'hl') {
        body.className = count > 1 ? 'body hl several' : 'body hl';
        body.innerHTML = glyph(kinds[0], 13);
    } else if (count > 1) body.className = 'body neutral';
    else if (look === 'dot') { body.className = 'body dot'; body.dataset.kind = kinds[0]; }
    else { body.className = 'body bare'; body.innerHTML = glyph(kinds[0], 14); }
    return body;
}

/** The body of a mark: the plain icon of today for one place, else the badge in the given shape. */
export function badgeBody(shape: Shape, kinds: PlaceCategory[], count: number, tileMax = TILE_MAX): HTMLElement {
    const body = document.createElement('span');
    const more = (shown: number) => (kinds.length > shown ? `<b>+${kinds.length - shown}</b>` : '');
    if (count === 1) {
        body.className = 'body disc';
        body.innerHTML = glyph(kinds[0]);
    } else if (kinds.length === 1 && shape === 'pill') {
        body.className = 'body pill group';
        body.innerHTML = `${glyph(kinds[0])}<b>${count}</b>`;
    } else if (kinds.length === 1) {
        // Several places of one kind: the icon with a second disc behind it.
        body.className = 'body disc several';
        body.innerHTML = glyph(kinds[0]);
    } else if (shape === 'pill') {
        body.className = 'body pill';
        body.innerHTML = kinds.slice(0, 4).map(kind => glyph(kind, 13)).join('') + more(4);
    } else if (shape === 'tile') {
        const shown = kinds.length > tileMax ? tileMax - 1 : kinds.length, cells = Math.min(kinds.length, tileMax);
        body.className = cells === 2 ? 'body tile row' : 'body tile';
        body.style.setProperty('--cols', String(cells <= 4 ? 2 : cells <= 6 ? 3 : 4));
        body.innerHTML = kinds.slice(0, shown).map(kind => glyph(kind)).join('') + more(shown);
    } else if (shape === 'stack') {
        const shown = kinds.length > 4 ? 3 : kinds.length;
        body.className = 'body stack';
        body.innerHTML = kinds.slice(0, shown).map(kind => `<span class="disc">${glyph(kind)}</span>`).join('') + (kinds.length > shown ? `<span class="disc">${more(shown)}</span>` : '');
    } else {
        const dots = Math.min(kinds.length - 1, 6);
        body.className = 'body disc ring';
        body.innerHTML = glyph(kinds[0]) + Array.from({ length: dots }, (_, index) => `<i style="--a:${(index - (dots - 1) / 2) * 26}deg"></i>`).join('');
    }
    return body;
}

const kindsOf = (places: PlaceFeature[]) => ORDER.filter(category => places.some(place => place.properties.category === category));
const nameOf = (place: PlaceFeature) => place.properties.name || poiKinds[place.properties.kind]?.label || placeCategories[place.properties.category].label;

type Mark = { look?: Look; key: string; lon: number; lat: number; places: PlaceFeature[]; kinds: PlaceCategory[]; element: HTMLButtonElement; body: HTMLElement; x: number; y: number; box?: string };

/** A clear image of the size of a mark (`proto-blank-W-H`): the map keeps its labels off the marks with it. */
export function blankIcon(id: string): { image: ImageData; pixelRatio: number } | null {
    const match = id.match(/^proto-blank-(\d+)-(\d+)$/);
    return match ? { image: new ImageData(Number(match[1]), Number(match[2])), pixelRatio: 1 } : null;
}

const NAMES = 'proto-names';

export class BadgeLayer {
    shape = badgeShape;
    /** The motions run this many times slower. */
    slow = SLOW;
    private readonly root = document.createElement('div');
    private marks = new Map<string, Mark>();
    private readonly leaving = new Set<Mark>();
    private wide = new Map<number, Node[]>();
    private near = new Map<number, Node[]>();
    /** The pinned kinds alone, for the zooms below minZoom. */
    private early = new Map<number, Node[]>();
    private level = -1;
    /** Quiet: the groups of each look. */
    private quiet: Record<Look, Map<number, Node[]>> = { glyph: new Map(), dot: new Map(), hl: new Map() };
    private list: { mark: Mark; element: HTMLElement; panel: HTMLElement } | undefined;
    private picked: { place: PlaceFeature; element: HTMLElement; preview: boolean; id?: string } | undefined;
    private press: [number, number] = [0, 0];

    constructor(private readonly map: maplibregl.Map, private readonly settings: { badgeRadius: number; touchRadius: number; split: number; minZoom: number; pinned: PlaceCategory[]; pinnedZoom: number; names: number; quiet?: Quiet; highlight?: PlaceCategory; dim: boolean },
        private readonly onPlace: (place: PlaceFeature) => string | undefined) {
        this.root.className = 'places-layer';
        for (const flag of ['halo', 'onedot']) if (query.has(flag)) this.root.classList.add(flag);
        map.getCanvasContainer().append(this.root);
        map.on('move', () => this.position());
        map.on('zoom', () => { if (Math.floor(map.getZoom()) !== this.level) this.update(true); });
        map.on('moveend', () => this.update(false));
        map.on('click', () => { this.closeList(); });
        window.addEventListener('keydown', event => { if (event.key === 'Escape' && this.list) { event.stopPropagation(); this.closeList(true); } }, true);
        document.addEventListener('pointerdown', event => { if (this.list && !(event.target instanceof Node && map.getContainer().contains(event.target))) this.closeList(); });
        // A pan that starts on a mark must not open it.
        this.root.addEventListener('pointerdown', event => { this.press = [event.clientX, event.clientY]; });
    }

    /** The names of the single places, from the names zoom. They stay a map layer, so the map keeps them off each other. */
    install(theme: 'light' | 'dark') {
        const { map } = this, dark = theme === 'dark';
        map.addSource(NAMES, { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
        map.addLayer({
            id: NAMES, type: 'symbol', source: NAMES,
            layout: { 'icon-image': ['get', 'box'], 'icon-allow-overlap': true, 'text-field': ['step', ['zoom'], '', this.settings.names, ['get', 'name']], 'text-font': ['Noto Sans Regular'], 'text-size': 11, 'text-anchor': 'top', 'text-offset': [0, 0.9], 'text-optional': true },
            paint: { 'text-color': dark ? '#f2efe3' : '#1c1b14', 'text-halo-color': dark ? '#201f17' : '#ffffff', 'text-halo-width': 1.2 },
        }, map.getLayer('contour-labels') ? 'contour-labels' : undefined);
        this.update(false);
    }

    setData(features: PlaceFeature[]) {
        const { badgeRadius, touchRadius, split, minZoom, pinned, pinnedZoom, quiet, highlight } = this.settings;
        if (quiet) {
            const others = features.filter(feature => feature.properties.category !== highlight);
            this.quiet = {
                hl: levels(features.filter(feature => feature.properties.category === highlight), RADIUS.hl, HL_ZOOM - 1, 19),
                glyph: levels(others, RADIUS.glyph, Math.min(minZoom, BARE) - 1, 19),
                dot: levels(others, RADIUS.dot, minZoom - 1, BARE - 1),
            };
            this.update(false);
            return;
        }
        this.early = levels(features.filter(feature => pinned.includes(feature.properties.category)), badgeRadius, pinnedZoom - 1, minZoom - 1);
        this.wide = levels(features, badgeRadius, minZoom - 1, split - 1);
        this.near = levels(features, touchRadius, split, 19);
        this.update(false);
    }

    /** For the pictures: another shape without a new page. */
    setShape(shape: Shape) {
        this.closeList();
        this.shape = shape;
        for (const mark of this.marks.values()) mark.element.remove();
        this.marks = new Map();
        this.update(false);
    }

    /** The app chose another place, or none: the marked icon goes. */
    selected(id: string | null) {
        if (this.picked && !this.picked.preview && this.picked.id && id !== this.picked.id) this.choose(null);
    }

    private nodes(level: number): [Node, Look?][] {
        const { split, minZoom, pinned, pinnedZoom, quiet, highlight, dim } = this.settings;
        if (quiet) {
            const zoom = this.map.getZoom(), at = Math.min(level, 20), out: [Node, Look?][] = [];
            if (zoom >= HL_ZOOM) out.push(...this.inView(this.quiet.hl.get(at)).map(node => [node, 'hl'] as [Node, Look]));
            const look: Look = quiet === 'dot' && level < BARE ? 'dot' : 'glyph';
            if (zoom >= (quiet === 'none' ? BARE : minZoom) && !(dim && highlight)) out.push(...this.inView(this.quiet[look].get(at)).map(node => [node, look] as [Node, Look]));
            return out;
        }
        const first = pinned.length ? pinnedZoom : minZoom;
        if (this.map.getZoom() < first) return [];
        const all = level < minZoom ? this.early.get(Math.max(level, first - 1)) : level < split ? this.wide.get(level) : this.near.get(Math.min(level, 20));
        return this.inView(all).map(node => [node]);
    }

    private inView(all: Node[] | undefined): Node[] {
        const bounds = this.map.getBounds(), x0 = worldX(bounds.getWest()), x1 = worldX(bounds.getEast()), y0 = worldY(bounds.getNorth()), y1 = worldY(bounds.getSouth());
        const padX = (x1 - x0) * 0.3, padY = (y1 - y0) * 0.3;
        return (all ?? []).filter(node => node.x > x0 - padX && node.x < x1 + padX && node.y > y0 - padY && node.y < y1 + padY);
    }

    private update(animate: boolean) {
        const level = Math.floor(this.map.getZoom());
        const motion = animate && !still() && this.level >= 0 && level !== this.level, zoomedIn = level > this.level;
        const before = this.marks, next = new Map<string, Mark>(), fresh: Mark[] = [];
        for (const [node, look] of this.nodes(level)) {
            const key = (look ?? '') + keyOf(node);
            let mark = before.get(key);
            if (!mark) fresh.push(mark = this.create(node, key, look));
            next.set(key, mark);
        }
        this.marks = next;
        this.level = level;
        this.position();
        const gone = [...before.values()].filter(mark => !next.has(mark.key));
        // The mark that holds a place, before the step and after it.
        const owner = (marks: Iterable<Mark>) => { const map = new Map<unknown, Mark>(); for (const mark of marks) for (const place of mark.places) map.set(place.id, mark); return map; };
        const from = motion && zoomedIn ? owner(gone) : null, into = motion && !zoomedIn ? owner(fresh) : null;
        for (const mark of gone) {
            if (this.list?.mark === mark) this.closeList();
            const target = into?.get(mark.places[0].id);
            if (!motion) { mark.element.remove(); continue; }
            // Zoom in: the old badge fades under the marks that leave it. Zoom out: the marks move into the new badge.
            this.leaving.add(mark);
            mark.element.style.pointerEvents = 'none';
            const move = target ? `translate(${target.x - mark.x}px, ${target.y - mark.y}px) scale(0.85)` : 'none';
            mark.body.animate([{ transform: 'none', opacity: 1 }, { transform: move, opacity: 0 }], { duration: (target ? SPLIT_MS : FADE_MS) * this.slow, easing: EASE_OUT, fill: 'forwards' })
                .finished.then(() => { this.leaving.delete(mark); mark.element.remove(); }, () => {});
        }
        for (const mark of fresh) {
            if (!motion) continue;
            const origin = from?.get(mark.places[0].id);
            const start = origin ? `translate(${origin.x - mark.x}px, ${origin.y - mark.y}px) scale(0.85)` : 'scale(0.9)';
            mark.body.animate([{ transform: start, opacity: 0 }, { transform: 'none', opacity: 1 }], { duration: SPLIT_MS * this.slow, easing: EASE_OUT });
        }
        for (const mark of fresh) mark.box = `proto-blank-${mark.body.offsetWidth}-${mark.body.offsetHeight}`;
        (this.map.getSource(NAMES) as GeoJSONSource | undefined)?.setData({
            type: 'FeatureCollection',
            features: [...next.values()].map(mark => ({ type: 'Feature', geometry: { type: 'Point', coordinates: [mark.lon, mark.lat] }, properties: { box: mark.box, name: mark.places.length === 1 ? mark.places[0].properties.name : '' } })),
        });
    }

    private create(node: Node, key: string, look?: Look): Mark {
        const element = document.createElement('button'), kinds = kindsOf(node.places);
        const body = look ? quietBody(look, kinds, node.places.length) : badgeBody(this.shape, kinds, node.places.length);
        element.type = 'button';
        element.className = look === 'hl' ? 'places-mark hl' : 'places-mark';
        element.setAttribute('aria-label', node.places.length === 1 ? nameOf(node.places[0]) : `${node.places.length} places: ${kinds.map(kind => placeCategories[kind].label).join(', ')}`);
        element.append(body);
        const mark: Mark = { look, key, lon: lonOf(node.x), lat: latOf(node.y), places: node.places, kinds, element, body, x: 0, y: 0 };
        element.addEventListener('click', event => {
            event.stopPropagation();
            if (Math.hypot(event.clientX - this.press[0], event.clientY - this.press[1]) > 5) return;
            if (mark.places.length === 1) { this.closeList(); this.pick(mark.places[0]); }
            else if (this.list?.mark === mark) this.closeList();
            else this.openList(mark);
        });
        this.root.append(element);
        return mark;
    }

    private at(lon: number, lat: number): [number, number] {
        const point = this.map.project([lon, lat]), ratio = devicePixelRatio;
        return [Math.round(point.x * ratio) / ratio, Math.round(point.y * ratio) / ratio];
    }

    private position() {
        for (const mark of [...this.marks.values(), ...this.leaving]) {
            [mark.x, mark.y] = this.at(mark.lon, mark.lat);
            mark.element.style.transform = `translate(${mark.x}px, ${mark.y}px)`;
        }
        if (this.list) this.list.element.style.transform = `translate(${this.list.mark.x}px, ${this.list.mark.y}px)`;
        if (this.picked) {
            const [x, y] = this.at(this.picked.place.properties.lon, this.picked.place.properties.lat);
            this.picked.element.style.transform = `translate(${x}px, ${y}px)`;
        }
    }

    // ---- The list of a badge.

    private openList(mark: Mark) {
        this.closeList();
        this.choose(null);
        // The rows scroll inside the panel. The panel itself has no scroll: Chromium drew a blank area on the map beside a panel that scrolls.
        const element = document.createElement('div'), panel = document.createElement('div'), rows = document.createElement('div');
        element.className = 'places-list';
        panel.className = 'panel';
        rows.className = 'rows';
        panel.tabIndex = -1;
        panel.setAttribute('role', 'dialog');
        panel.setAttribute('aria-label', `${mark.places.length} places`);
        panel.append(rows);
        const places = [...mark.places].sort((a, b) => rank(a.properties.category) - rank(b.properties.category) || nameOf(a).localeCompare(nameOf(b)));
        places.forEach((place, index) => {
            const row = document.createElement('button');
            row.type = 'button';
            if (index && places[index - 1].properties.category !== place.properties.category) row.className = 'kind-start';
            row.innerHTML = glyph(place.properties.category, 14);
            const name = document.createElement('span');
            name.textContent = nameOf(place);
            row.append(name);
            row.addEventListener('click', () => { this.closeList(); this.pick(place, mark); });
            // Which of the places is this one: the row shows its icon on the map.
            row.addEventListener('pointerenter', () => this.choose(place, true));
            row.addEventListener('pointerleave', () => { if (this.picked?.preview) this.choose(null); });
            rows.append(row);
        });
        // The map must not pan, zoom or handle a click under the list.
        for (const type of ['pointerdown', 'mousedown', 'touchstart', 'dblclick', 'wheel', 'click', 'contextmenu']) panel.addEventListener(type, event => event.stopPropagation());
        element.append(panel);
        this.root.append(element);
        mark.element.setAttribute('aria-expanded', 'true');
        this.list = { mark, element, panel };
        this.position();
        // Below the badge, or above it when there is more room there; inside the map from side to side.
        const box = this.map.getContainer().getBoundingClientRect(), badge = mark.body.getBoundingClientRect();
        const gap = 6, half = badge.height / 2 + gap, roomBelow = box.height - mark.y - half - 12, roomAbove = mark.y - half - 12;
        const above = roomBelow < Math.min(panel.offsetHeight, 200) && roomAbove > roomBelow;
        rows.style.maxHeight = `${Math.max(120, Math.min(312, above ? roomAbove : roomBelow)) - 8}px`;
        const width = panel.offsetWidth, height = panel.offsetHeight;
        const left = Math.max(8 - mark.x, Math.min(-width / 2, box.width - 8 - width - mark.x)), top = above ? -half - height : half;
        panel.style.left = `${left}px`;
        panel.style.top = `${top}px`;
        // The list grows out of the badge.
        panel.style.transformOrigin = `${-left}px ${-top}px`;
        if (!still()) panel.animate([{ transform: 'scale(0.9)', opacity: 0 }, { transform: 'none', opacity: 1 }], { duration: LIST_IN * this.slow, easing: EASE_OUT });
        panel.focus({ preventScroll: true });
    }

    private closeList(refocus = false) {
        const list = this.list;
        if (!list) return;
        this.list = undefined;
        list.mark.element.removeAttribute('aria-expanded');
        if (this.picked?.preview) this.choose(null);
        if (refocus) list.mark.element.focus({ preventScroll: true });
        list.panel.style.pointerEvents = 'none';
        if (still()) { list.element.remove(); return; }
        list.panel.animate([{ transform: 'none', opacity: 1 }, { transform: 'scale(0.96)', opacity: 0 }], { duration: LIST_OUT * this.slow, easing: EASE_OUT, fill: 'forwards' })
            .finished.then(() => list.element.remove(), () => list.element.remove());
    }

    // ---- The marked icon of the chosen place.

    private choose(place: PlaceFeature | null, preview = false, from?: Mark) {
        this.picked?.element.remove();
        this.picked = undefined;
        if (!place) return;
        const element = document.createElement('div'), body = badgeBody(this.shape, [place.properties.category], 1);
        element.className = preview ? 'places-mark chosen preview' : 'places-mark chosen';
        element.append(body);
        this.root.append(element);
        this.picked = { place, element, preview };
        this.position();
        if (from && !still()) {
            const [x, y] = this.at(place.properties.lon, place.properties.lat);
            body.animate([{ transform: `translate(${from.x - x}px, ${from.y - y}px) scale(0.85)`, opacity: 0 }, { transform: 'none', opacity: 1 }], { duration: SPLIT_MS * this.slow, easing: EASE_OUT });
        }
    }

    private pick(place: PlaceFeature, from?: Mark) {
        this.choose(place, false, from);
        const id = this.onPlace(place);
        if (this.picked) this.picked.id = id;
    }

    // ---- For the pictures and the measurements.

    /** Each mark of the view with its box on the map, in CSS px. */
    info(): { look: string; key: string; count: number; kinds: string; x: number; y: number; w: number; h: number; lon: number; lat: number }[] {
        const box = this.map.getContainer().getBoundingClientRect();
        return [...this.marks.values()].map(mark => {
            const rect = mark.body.getBoundingClientRect();
            return { look: mark.look === 'hl' ? 'hl' : mark.look && mark.places.length > 1 ? 'neutral' : mark.look ?? 'badge', key: mark.key, count: mark.places.length, kinds: mark.kinds.join('.'), x: mark.x, y: mark.y, w: rect.width, h: rect.height, lon: mark.lon, lat: mark.lat };
        }).filter(item => item.x >= 0 && item.x <= box.width && item.y >= 0 && item.y <= box.height);
    }

    /** Shows only the given specimens, in a row at fixed places of the screen. */
    specimen(items: { kinds: PlaceCategory[]; count: number; x: number; shape?: Shape; tileMax?: number; look?: Look }[], y: number) {
        this.root.classList.add('specimen');
        this.root.querySelectorAll('.places-specimen').forEach(element => element.remove());
        items.forEach(item => {
            const element = document.createElement('div');
            element.className = 'places-mark places-specimen';
            element.style.transform = `translate(${item.x}px, ${y}px)`;
            if (item.look) element.append(quietBody(item.look, item.kinds, item.count));
            else element.append(badgeBody(item.shape ?? this.shape, item.kinds, item.count, item.tileMax));
            this.root.append(element);
        });
    }
}
