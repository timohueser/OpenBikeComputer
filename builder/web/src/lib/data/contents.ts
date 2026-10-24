// What a bake contains, as a person names it. A card switches the optional layers of
// `obc data update` that only make sense together. The bake adds what a card reads, such as the
// terrain; `shownBy` names the cards that show it, without which it has no reader.
import type { Bbox } from "./areas";
import type { SourceRow } from "./sources";

export interface Content {
    id: string;
    title: string;
    about: string;
    layers: string[];
    /** Sources that need a credential on this machine. */
    credentials: string[];
    /** A prepared family, such as `forest-reference`: its members that meet the area. `required`
     *  cards cannot bake without one; the others fall back to coarser data. */
    reference?: { family: string; required: boolean; without: string };
    /** The cards that show it; it can be on only while one of them is. */
    shownBy?: string[];
    /** Whether it reads the terrain, which a national model makes finer where it covers the area. */
    terrain?: boolean;
}

/** The terrain that the cards with `terrain` read: a national model where one is prepared, else
 *  Copernicus 30 m. */
export const TERRAIN: Content = {
    id: "terrain",
    title: "Terrain",
    about: "",
    layers: [],
    credentials: [],
    reference: { family: "terrain-reference", required: false, without: "Copernicus 30 m" },
};

export const CONTENTS: Content[] = [
    {
        id: "device",
        title: "Bike computer maps",
        about: "Map, terrain, landmarks and peaks",
        layers: ["device"],
        credentials: [],
        terrain: true,
    },
    { id: "map", title: "Planner map", about: "Base map, terrain shading and places", layers: ["map"], credentials: [], terrain: true },
    { id: "routing", title: "Routing", about: "Routes for each bike profile", layers: ["routing"], credentials: [] },
    { id: "search", title: "Search", about: "Places and addresses by name", layers: ["search"], credentials: [] },
    {
        id: "trailview",
        title: "Trail View",
        about: "3D ground, roads, water and colour",
        layers: ["trailview-features", "trailview-rules", "trailview-ground", "trailview-colour"],
        credentials: ["sentinel-2-mosaics"],
        terrain: true,
    },
    {
        id: "trailview-canopy",
        title: "Trail View forest",
        about: "Tree height and leaf type",
        layers: ["trailview-canopy"],
        credentials: [],
        reference: { family: "forest-reference", required: true, without: "no trees" },
        shownBy: ["trailview"],
    },
    { id: "sun", title: "Sun", about: "Sunrise and sunset behind the terrain", layers: ["sun"], credentials: [], shownBy: ["map"], terrain: true },
    { id: "snow", title: "Snow", about: "Typical snow cover through the year", layers: ["snow"], credentials: ["hr-wsi"], shownBy: ["map", "trailview"] },
    { id: "climate", title: "Climate", about: "Monthly temperature and rain", layers: ["climate"], credentials: ["era5-land"], shownBy: ["map"] },
];

const title = (id: string) => CONTENTS.find((content) => content.id === id)?.title ?? id;

/** Why `content` cannot be on with the cards `on`, or `null`: none of the cards that show it is on. */
export function unshown(content: Content, on: string[]): string | null {
    if (!content.shownBy || content.shownBy.some((id) => on.includes(id))) return null;
    return `Shows in ${new Intl.ListFormat("en", { type: "disjunction" }).format(content.shownBy.map(title))}: switch it on first.`;
}

/** The titles of the cards that `content` shows. */
export const shows = (content: Content): string[] => CONTENTS.filter((other) => other.shownBy?.includes(content.id)).map((other) => other.title);

/** `on` with `id` switched, and without the cards that no card that stays on shows. */
export function toggled(on: string[], id: string, checked: boolean): string[] {
    const shown = (ids: string[]) => ids.filter((at) => !unshown(CONTENTS.find((content) => content.id === at)!, ids));
    let next = checked ? [...on, id] : on.filter((at) => at !== id);
    // A card that goes can leave a card that only it showed: drop until nothing more goes.
    for (let kept = shown(next); kept.length < next.length; kept = shown(next)) next = kept;
    return next;
}

/** A prepared reference that meets the area: its member, such as `terrain-reference/ch`, and the
 *  target of its newest version, `null` while it has none. */
export interface Reference {
    source: string;
    use: string;
    version: { version: string; target: string } | null;
}

export interface Needs {
    /** Credentials that this machine lacks, by source. */
    credentials: string[];
    /** The members of the reference family that meet the area. */
    references: Reference[];
    /** Whether the card can bake here. */
    ready: boolean;
}

const meets = (extent: [number, number, number, number], area: Bbox) =>
    extent[0] < area.east && area.west < extent[2] && extent[1] < area.north && area.south < extent[3];

/** Whether member `id`, such as `terrain-reference/de-by`, is of one of `countries`; any member is
 *  when the area names no countries, such as a drawn box. */
const ofCountry = (id: string, countries: string[]) => !countries.length || countries.includes(id.split("/")[1].split("-")[0].toUpperCase());

/** What `content` needs for `area` in `countries` on this machine, from `obc data sources`. A bake
 *  reads a version from `prod` first, so a `prod` version wins over a `local` one. */
export function needs(content: Content, area: Bbox | null, sources: SourceRow[], countries: string[] = []): Needs {
    const credentials = content.credentials.filter((id) => sources.find((source) => source.id === id)?.credential_missing);
    const family = content.reference?.family;
    const references: Reference[] =
        family && area
            ? sources
                  .filter((source) => source.id.startsWith(`${family}/`) && source.extent && meets(source.extent, area) && ofCountry(source.id, countries))
                  .map((source) => {
                      const target = ["prod", "local"].find((name) => source.prepared[name]?.versions.length);
                      const newest = target ? source.prepared[target].versions.at(-1)! : null;
                      return { source: source.id, use: source.use ?? source.id, version: newest && { version: newest.version, target: target! } };
                  })
            : [];
    const covered = references.some((reference) => reference.version);
    return { credentials, references, ready: !credentials.length && (!content.reference?.required || covered) };
}

/** The `--add` and `--remove` flags that switch on exactly the layers of the cards in `on`. */
export function layerFlags(on: string[]): [string, string][] {
    return CONTENTS.flatMap((content) => content.layers.map((layer): [string, string] => [on.includes(content.id) ? "--add" : "--remove", layer]));
}

/** The cards whose layers are all in `layers`, as the `--add` flags of a run name them. */
export function cardsOf(layers: string[]): string[] {
    return CONTENTS.filter((content) => content.layers.length && content.layers.every((layer) => layers.includes(layer))).map((content) => content.id);
}
