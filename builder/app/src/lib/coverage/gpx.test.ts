import { describe, expect, it } from "vitest";
import { GpxError, MAX_ROUTE_POINTS, parseGpx } from "./gpx";

const HEADER = '<?xml version="1.0" encoding="UTF-8"?><gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1">';

function track(points: string, name?: string): string {
    return `${HEADER}<trk>${name ? `<name>${name}</name>` : ""}<trkseg>${points}</trkseg></trk></gpx>`;
}

describe("parseGpx", () => {
    it("reads track points into integer microdegrees", () => {
        const route = parseGpx(
            track('<trkpt lat="47.5" lon="7.75"><ele>210</ele></trkpt><trkpt lat="47.6" lon="7.8"/>'),
            "file",
        );
        expect(route.points).toEqual([
            { lat: 47_500_000, lon: 7_750_000 },
            { lat: 47_600_000, lon: 7_800_000 },
        ]);
    });

    it("accepts either attribute order, either quote style, self-closing or not", () => {
        const route = parseGpx(
            track("<trkpt lon='8.0' lat='47.0'></trkpt><trkpt lat=\"47.1\" lon=\"8.1\"/>"),
            "file",
        );
        expect(route.points).toHaveLength(2);
        expect(route.points[0]).toEqual({ lat: 47_000_000, lon: 8_000_000 });
    });

    it("joins multiple segments — a recorder losing fix is one ride, not two", () => {
        const body = `${HEADER}<trk><trkseg><trkpt lat="47" lon="8"/></trkseg><trkseg><trkpt lat="47.1" lon="8"/></trkseg></trk></gpx>`;
        expect(parseGpx(body, "file").points).toHaveLength(2);
    });

    it("falls back to route points when there are no track points", () => {
        const body = `${HEADER}<rte><rtept lat="47" lon="8"/><rtept lat="47.1" lon="8"/></rte></gpx>`;
        expect(parseGpx(body, "file").points).toHaveLength(2);
    });

    it("names the route from the track, un-escaping XML entities", () => {
        const body = `${HEADER}<metadata><name>The File</name></metadata><trk><name>Rhein &amp; R&#xF6;n</name><trkseg><trkpt lat="47" lon="8"/><trkpt lat="47.1" lon="8"/></trkseg></trk></gpx>`;
        // Only the five predefined entities are un-escaped; numeric references
        // stay as-is, which is the honest limit of a scanner.
        expect(parseGpx(body, "file").name).toBe("Rhein & R&#xF6;n");
    });

    it("prefers the track's own name over the file-level metadata name", () => {
        const body = `${HEADER}<metadata><name>File Name</name></metadata><trk><name>Ride Name</name><trkseg><trkpt lat="47" lon="8"/><trkpt lat="47.1" lon="8"/></trkseg></trk></gpx>`;
        expect(parseGpx(body, "x").name).toBe("Ride Name");
    });

    it("uses the metadata name when the track has none, and the fallback last", () => {
        const named = `${HEADER}<metadata><name>File Name</name></metadata><trk><trkseg><trkpt lat="47" lon="8"/><trkpt lat="47.1" lon="8"/></trkseg></trk></gpx>`;
        expect(parseGpx(named, "fallback").name).toBe("File Name");
        expect(parseGpx(track('<trkpt lat="47" lon="8"/><trkpt lat="47.1" lon="8"/>'), "fallback").name).toBe(
            "fallback",
        );
    });

    it("measures a degree of latitude as ~111 km", () => {
        const route = parseGpx(track('<trkpt lat="47" lon="8"/><trkpt lat="48" lon="8"/>'), "x");
        expect(route.distanceKm).toBeGreaterThan(110);
        expect(route.distanceKm).toBeLessThan(113);
    });

    it("decimates a dense track to the ceiling, keeping both ends", () => {
        const points = Array.from(
            { length: 10_000 },
            (_, k) => `<trkpt lat="${(47 + k * 0.0001).toFixed(4)}" lon="8"/>`,
        ).join("");
        const route = parseGpx(track(points), "x");
        expect(route.points).toHaveLength(MAX_ROUTE_POINTS);
        expect(route.points[0].lat).toBe(47_000_000);
        expect(route.points.at(-1)!.lat).toBe(47_999_900);
    });

    it("refuses a file with no points, naming the likely cause", () => {
        expect(() => parseGpx("<html>not gpx</html>", "x")).toThrow(GpxError);
        expect(() => parseGpx("<html>not gpx</html>", "x")).toThrow(/is this a GPX file/);
    });

    it("refuses a single point — that is a waypoint, not a route", () => {
        expect(() => parseGpx(track('<trkpt lat="47" lon="8"/>'), "x")).toThrow(/fewer than two/);
    });

    it("counts malformed coordinates instead of silently dropping the file's story", () => {
        const body = track('<trkpt lat="none" lon="8"/><trkpt lat="47"/>');
        expect(() => parseGpx(body, "x")).toThrow(/2 of 2 carried malformed coordinates/);
    });

    it("drops out-of-range coordinates as malformed rather than folding them onto the globe", () => {
        const body = track('<trkpt lat="91" lon="8"/><trkpt lat="47" lon="181"/>');
        expect(() => parseGpx(body, "x")).toThrow(GpxError);
    });
});
