export type Coordinate = [number, number];

export interface MapPoint {
    id: string;
    coordinate: Coordinate;
    label: string;
    kind: "start" | "finish" | "via" | "night" | "place" | "waypoint" | "detour" | "pass" | "marker";
    appearance?: "hotel" | "camp" | "suggested";
    color?: string;
    markerLabel?: string;
    draggable?: boolean;
}
