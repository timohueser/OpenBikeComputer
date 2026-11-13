import { describe, expect, it } from "vitest";
import { FitError, readFit, writeFitCourse, writeFitRide, type FitCourse, type FitRide } from "./fit";

describe("FIT", () => {
    it("reads a written ride with the same values", () => {
        const ride: FitRide = {
            sport: "gravel",
            samples: [
                { timeS: 1_760_000_000, lat: 46_571_234, lon: 8_100_000, segmentStart: true, elevationM: 2164.4 },
                {
                    timeS: 1_760_000_001, lat: 46_571_300, lon: 8_100_010, segmentStart: false,
                    speedMps: 7.25, hrBpm: 142, cadenceRpm: 88, powerW: 231, temperatureC: -3,
                },
                { timeS: 1_760_000_400, lat: 46_571_400, lon: 8_100_020, segmentStart: true },
            ],
        };
        expect(readFit(writeFitRide(ride), "x")).toEqual(ride);
    });

    it("reads a written course with the same values, and names it from the fallback when it has no name", () => {
        const course: FitCourse = {
            name: "",
            points: [{ lat: 46_560_000, lon: 8_400_000, ele: 1708.2 }, { lat: 46_570_000, lon: 8_400_000 }],
            waypoints: [{ lat: 46_570_000, lon: 8_400_000, name: "Wasser" }],
            elapsedS: 3600,
        };
        expect(readFit(writeFitCourse(course), "Furka")).toEqual({ ...course, name: "Furka" });
    });

    it("throws a FitError with a code", () => {
        const read = () => readFit(new TextEncoder().encode("<gpx></gpx>"), "x");
        expect(read).toThrow(FitError);
        expect(read).toThrow(expect.objectContaining({ code: "not-fit" }));
    });
});
