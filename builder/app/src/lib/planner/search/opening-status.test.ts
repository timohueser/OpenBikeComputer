import { describe, expect, it } from 'vitest';
import { openingStatus } from './opening-status';
import type { HoursStatus } from './types';
const now=Date.parse('2026-09-28T10:33:00Z');
const value: HoursStatus={state:'open',checkedAt:now,validUntil:now+300_000,closesAt:now+27*60_000};
describe('current opening badge',()=>{
    it('shows a minute countdown only while open and less than one hour from closing',()=>{
        expect(openingStatus(value,now)).toEqual({state:'open',label:'Open',closesIn:27});
        expect(openingStatus({...value,closesAt:now+60*60_000},now).closesIn).toBeNull();
        expect(openingStatus({...value,closesAt:now+20_000},now).closesIn).toBe(1);
        expect(openingStatus({...value,state:'closed'},now)).toEqual({state:'closed',label:'Closed',closesIn:null});
    });
    it('does not keep an old open badge after expiry or a closing boundary',()=>{
        expect(openingStatus(value,value.validUntil)).toEqual({state:'unknown',label:'Hours unknown',closesIn:null});
        expect(openingStatus({...value,validUntil:now+20_000,closesAt:now+20_000},now+20_000).state).toBe('unknown');
    });
});
