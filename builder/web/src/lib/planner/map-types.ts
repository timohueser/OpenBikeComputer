import type { PlaceCategory } from "./poi-kinds";

export type Coordinate = [number, number];

export interface MapPoint {
    id: string;
    coordinate: Coordinate;
    label: string;
    kind: "start" | "finish" | "via" | "night" | "place" | "waypoint" | "detour" | "pass" | "marker" | "dayend";
    /** A `suggested` pin is not part of the route yet; a `moved` day end was dragged by the rider. */
    appearance?: "suggested" | "moved";
    /** The place category a `place` pin shows. */
    category?: PlaceCategory;
    color?: string;
    markerLabel?: string;
    /** A fixed point never moves by drag. Places are always fixed. */
    fixed?: boolean;
    /** The night a `dayend` pin ends. */
    night?: number;
}

/** One stretch of the drawn route: a leg, cut at day ends. */
export interface MapSegment {
    coordinates: Coordinate[];
    color: string;
    legEndId: string;
    leg: import("./editor").LegMode;
}
