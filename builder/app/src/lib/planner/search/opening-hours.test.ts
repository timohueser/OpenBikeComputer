import { describe, expect, it } from 'vitest';
import { openingHoursRows } from './opening-hours';
import { asPlace } from './presentation';

describe('opening hours presentation', () => {
    it('separates weekday ranges, split shifts, and a closed holiday rule', () => {
        expect(openingHoursRows('Mo-Fr 08:30-13:00,15:00-18:30; Sa 08:30-13:00; PH off')).toEqual([
            {days:'Mon–Fri',periods:['08:30–13:00','15:00–18:30']},
            {days:'Sat',periods:['08:30–13:00']},
            {days:'Public holidays',periods:['Closed']},
        ]);
    });
    it('distinguishes weekday lists from commas between complete rules', () => {
        expect(openingHoursRows('Mo,We,Th 08:30-18:30; Tu,Fr 08:00-18:30, Sa 08:30-14:00')).toEqual([
            {days:'Mon, Wed, Thu',periods:['08:30–18:30']},
            {days:'Tue, Fri',periods:['08:00–18:30']},
            {days:'Sat',periods:['08:30–14:00']},
        ]);
    });
    it('keeps overnight ranges and explicit all-day opening without inferring missing days', () => {
        expect(openingHoursRows('Fr-Sa 18:00-02:00')).toEqual([{days:'Fri–Sat',periods:['18:00–02:00']}]);
        expect(openingHoursRows('24/7')).toEqual([{days:'Every day',periods:['Open 24 hours']}]);
    });
    it('uses the complete original expression when dates, solar times, comments or fallback rules need interpretation', () => {
        for (const raw of ['Mo-Fr 08:00-17:00; Dec 24 off','sunrise-sunset','Mo-Fr 08:00-12:00 || "by appointment"','Mo-Fr 10:00-17:00; PH open "call ahead"','']) expect(openingHoursRows(raw)).toBeNull();
    });
    it('carries hours and locality into the same place used by list and map', () => {
        const place=asPlace({source:'test',name:'Pharmacy',kind:'pharmacy',city:'Teningen',region:'Baden-Württemberg',lon:7.81,lat:48.1,distance:1,precision:'place',opening_hours:'Mo-Fr 08:30-13:00'});
        expect(place.openingHours).toBe('Mo-Fr 08:30-13:00');
        expect(place.locality).toBe('Teningen');
        expect(place.description).toBe('');
    });
});
