import { expect, it } from "vitest";
import { search, type Areas } from "./areas";

const box = { west: 0, south: 0, east: 1, north: 1 };
const areas: Areas = {
    index: "2026-10-01",
    regions: [
        { id: "dach", name: "Germany, Austria and Switzerland", countries: [], kind: "box", box },
        { id: "bw-ch", name: "Baden-Württemberg and Switzerland", countries: [], kind: "box", box },
    ],
    areas: [
        { id: "europe", name: "Europe", parent: null, countries: [], bounds: box },
        { id: "europe/switzerland", name: "Switzerland", parent: "europe", countries: ["CH"], bounds: box },
        { id: "europe/austria", name: "Austria", parent: "europe", countries: ["AT"], bounds: box },
    ],
};

it("ranks an exact name before a word and a substring, saved areas first within a rank", () => {
    expect(search("switzerland", areas).map((hit) => hit.id)).toEqual(["europe/switzerland", "dach", "bw-ch"]);
    expect(search("austria", areas)[0].id).toBe("europe/austria");
    expect(search("witzer", areas).map((hit) => hit.id)).toEqual(["dach", "bw-ch", "europe/switzerland"]);
});
