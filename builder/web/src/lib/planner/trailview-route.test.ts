import { describe, expect, it } from 'vitest';
import { dayColor } from './day-colors';
import type { Coordinate } from './geo';
import type { Surface } from './routing';
import { routeColours, surfaceColors } from './trailview-route';

describe('routeColours', () => {
    it('colours each edge by the day that rides its middle and by its surface', () => {
        // Three edges of about 111 m, 222 m and 111 m north.
        const surfaces: (Surface | null)[] = ['Paved', null, 'Gravel'];
        const line = { coordinates: [[8, 46], [8, 46.001], [8, 46.003], [8, 46.004]] as Coordinate[], elevation: [500, 500, 500, 500], edges: { surfaces } };
        const colours = routeColours(line, [{ number: 1, to: 0.4 }, { number: 2, to: 1 }]);
        expect(colours.day).toEqual([dayColor(1), dayColor(2), dayColor(2)]);
        expect(colours.surface).toEqual([surfaceColors.Paved, surfaceColors.Unknown, surfaceColors.Gravel]);
        expect(colours.grade).toHaveLength(3);
    });
});
