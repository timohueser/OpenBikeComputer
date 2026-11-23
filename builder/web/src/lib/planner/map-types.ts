import type { PlaceCategory } from "./poi-kinds";

export type Coordinate = [number, number];

export interface MapPoint {
    id: string;
    coordinate: Coordinate;
    label: string;
    kind: "start" | "finish" | "via" | "night" | "place" | "waypoint" | "detour" | "pass" | "marker" | "dayend";
    /** A `suggested` pin is not part of the route yet; a `moved` day end was dragged by the rider. */
    appearance?: "suggested" | "moved" | "option";
    /** The place category a `place` or `night` pin shows. */
    category?: PlaceCategory;
    color?: string;
    markerLabel?: string;
    /** A fixed point never moves by drag. Places are always fixed. */
    fixed?: boolean;
    /** The night a `night` or `dayend` pin ends. */
    night?: number;
    /** The riding day of a route point. */
    day?: number;
    /** The stop of a night that a lone place is: a small pin with its name, which lights with its stop. */
    stop?: string;
    /** The place selected in the open stop of a night: an ink pin with its name. */
    named?: boolean;
}

/** A town of a night: the map draws an area over its places to sleep, with its name. */
export interface MapTown {
    id: string;
    name: string;
    places: Coordinate[];
    /** The open town is larger and takes no pointer. */
    open?: boolean;
}

/** One stretch of the drawn route: a leg, cut at day ends. */
export interface MapSegment {
    coordinates: Coordinate[];
    color: string;
    legEndId: string;
    /** The number of its day. */
    day: number;
    /** A gap is a leg with no legal path, drawn as a dashed straight line. */
    leg: import("./editor").LegMode | "gap";
}
