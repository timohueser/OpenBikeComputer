import { expect, it } from 'vitest';
import { dropPoint, HANG_PX } from './trailview';

it('the binoculars drop at their bottom, which lags behind a move to the right', () => {
    expect(dropPoint([100, 200], 0)).toEqual([100, 200 + HANG_PX]);
    const [x, y] = dropPoint([100, 200], 30);
    expect(x).toBeCloseTo(100 - HANG_PX / 2);
    expect(y).toBeCloseTo(200 + HANG_PX * Math.cos(Math.PI / 6));
});
