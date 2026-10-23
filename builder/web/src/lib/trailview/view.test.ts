import { describe, expect, it } from 'vitest';
import { defaultView, formatView, parseView, type View } from './view';

const fallback = defaultView([8.37, 46.765, 8.46, 46.83], new Date(2026, 7, 15));

describe('the view in the address', () => {
    it('keeps a walk and a flight through the address, and leaves the lab', () => {
        const walk: View = { ...fallback, lat: 46.8205, lon: 8.403, heading: 160, pitch: 5, date: '2026-08-15', time: '15:00', debug: 'lod' };
        const search = formatView(walk, '?lab=a:x,b:y&lat=1');
        expect(search).toBe('?lab=a:x,b:y&lat=46.820500&lon=8.403000&heading=160.0&pitch=5.0&eye=1.6&date=2026-08-15&time=15:00&weather=sunny&debug=lod');
        expect(parseView(search, fallback)).toEqual(walk);
        const fly: View = { ...walk, mode: 'fly', height: 60000, debug: 'off' };
        expect(parseView(formatView(fly), fallback)).toEqual(fly);
    });
});
