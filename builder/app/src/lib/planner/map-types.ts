export type Coordinate = [number, number];

export interface MapPoint {
    id: string;
    coordinate: Coordinate;
    label: string;
    kind: "start" | "finish" | "via" | "night" | "place" | "waypoint" | "detour" | "pass" | "marker" | "dayend";
    appearance?: "hotel" | "camp" | "suggested";
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
    leg: "routed" | "straight" | "drawn";
}

/** A place from the basemap `pois` layer. */
export interface MapPoi {
    id: string;
    kind: string;
    label: string;
    coordinate: Coordinate;
}
