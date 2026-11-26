import { describe, expect, it } from 'vitest';
import { BUDGETS, PHONE_CAPS, type Budget } from './budget';
import { ARCHIVES, WORKERS as GROUND_WORKERS } from './stream/ground';

describe('the phone budget', () => {
    it('keeps the byte caps of the phone within PHONE_CAPS', () => {
        const { caps, planWorkers } = BUDGETS.phone;
        // A cap counts once, but where each worker or archive has its own; a new cap counts once until it is named here.
        const copies: Partial<Record<keyof Budget['caps'], number>> = { groundDecode: GROUND_WORKERS, planHeap: planWorkers, groundBodies: ARCHIVES };
        const sum = Object.entries(caps).reduce((total, [name, mib]) => total + mib * (copies[name as keyof Budget['caps']] ?? 1), 0);
        expect(sum).toBeLessThanOrEqual(PHONE_CAPS);
    });
});
