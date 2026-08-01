/**
 * The shared check (#1002): one request, made only when asked, and a prompt that remembers what the
 * rider already answered.
 *
 * Two of these are behaviours rather than conveniences. **One fetch** is why the module exists at
 * all — the card and the prompt both want the answer, and two surfaces must not become two
 * requests. **Nothing until `ensure`** is the privacy rule the card's `onMount` comment used to
 * carry: constructing the store, reading it, rendering against it must never reach the network,
 * because the only thing that licenses the request is a connected device.
 */

import { describe, expect, it } from "vitest";

import { FirmwareCheck, LEDGER_CAP, type LedgerStorage } from "./check.svelte";

const SERIAL = "00112233AABBCCDD";
const OTHER_SERIAL = "FFEEDDCCBBAA9988";

const MANIFEST = JSON.stringify({
    version: "1.4.0",
    size: 812_345,
    sha256: "a".repeat(64),
    url: "https://updates.openbikecomputer.com/fw/UPDATE.BIN",
});

/** A `fetch` that answers one canned response and counts how often it was called. */
function stubFetch(status: number, body = "") {
    const calls: string[] = [];
    const fetch = (async (input: RequestInfo | URL) => {
        calls.push(String(input));
        return { status, ok: status >= 200 && status < 300, text: async () => body } as Response;
    }) as typeof globalThis.fetch;
    return { fetch, calls };
}

/** The storage seam as a Map, so none of this touches a browser global. */
function memoryLedger(seed: Record<string, string> = {}): LedgerStorage & { map: Map<string, string> } {
    const map = new Map(Object.entries(seed));
    return {
        map,
        get: (key) => map.get(key) ?? null,
        set: (key, value) => void map.set(key, value),
    };
}

describe("the check", () => {
    it("makes no request until it is asked to", async () => {
        const { calls } = stubFetch(200, MANIFEST);
        const check = new FirmwareCheck(memoryLedger());
        // Everything a surface does before a device connects: read the state, ask for an offer.
        expect(check.release).toBeNull();
        expect(check.checked).toBe(false);
        expect(check.offer(SERIAL, "1.3.0")).toBeNull();
        expect(calls).toEqual([]);
    });

    it("fetches once however many surfaces ask", async () => {
        const { fetch, calls } = stubFetch(200, MANIFEST);
        const check = new FirmwareCheck(memoryLedger());
        await Promise.all([check.ensure({ fetch }), check.ensure({ fetch })]);
        await check.ensure({ fetch });
        expect(calls).toHaveLength(1);
        expect(check.release?.version).toBe("1.4.0");
        expect(check.failed).toBe(false);
    });

    it("treats nothing-published as an answer, not a failure", async () => {
        const { fetch } = stubFetch(404);
        const check = new FirmwareCheck(memoryLedger());
        await check.ensure({ fetch });
        expect(check.release).toBeNull();
        expect(check.failed).toBe(false);
        expect(check.checked).toBe(true);
    });

    it("records a failed check without throwing at its callers", async () => {
        const { fetch } = stubFetch(500);
        const check = new FirmwareCheck(memoryLedger());
        await expect(check.ensure({ fetch })).resolves.toBeUndefined();
        expect(check.failed).toBe(true);
        expect(check.checked).toBe(true);
    });
});

describe("what the prompt is offered", () => {
    async function checked(): Promise<FirmwareCheck> {
        const check = new FirmwareCheck(memoryLedger());
        await check.ensure({ fetch: stubFetch(200, MANIFEST).fetch });
        return check;
    }

    it("offers an older device the published release", async () => {
        const check = await checked();
        expect(check.offer(SERIAL, "1.3.0")?.version).toBe("1.4.0");
    });

    it("says nothing for every state that is not 'available'", async () => {
        const check = await checked();
        expect(check.offer(SERIAL, "1.4.0+deadbee"), "current").toBeNull();
        expect(check.offer(SERIAL, "1.5.0"), "ahead").toBeNull();
        // #773's locked refusal, and the one that would be worst as a popup: a probe-flashed
        // device is never told to update.
        expect(check.offer(SERIAL, "abc1234"), "a dev build").toBeNull();
        expect(check.offer(SERIAL, null), "nothing reported").toBeNull();
        expect(check.offer(null, "1.3.0"), "no serial to scope an answer to").toBeNull();
    });

    it("says nothing before the check has run", () => {
        const check = new FirmwareCheck(memoryLedger());
        expect(check.offer(SERIAL, "1.3.0")).toBeNull();
    });
});

describe("the answered ledger", () => {
    async function checked(storage: LedgerStorage): Promise<FirmwareCheck> {
        const check = new FirmwareCheck(storage);
        await check.ensure({ fetch: stubFetch(200, MANIFEST).fetch });
        return check;
    }

    it("asks once per (device, version)", async () => {
        const check = await checked(memoryLedger());
        check.answer(SERIAL, "1.4.0");
        expect(check.offer(SERIAL, "1.3.0")).toBeNull();
        // A different device is a different question…
        expect(check.offer(OTHER_SERIAL, "1.3.0")?.version).toBe("1.4.0");
        // …and so is a different version, which is what keeps the next release from being silent.
        expect(check.isAnswered(SERIAL, "1.5.0")).toBe(false);
    });

    it("does not re-ask after a channel rollback or regress the ledger", async () => {
        const check = await checked(memoryLedger());
        check.answer(SERIAL, "1.5.0");
        check.answer(SERIAL, "1.4.0"); // a late, older surface completion
        expect(check.isAnswered(SERIAL, "1.5.0")).toBe(true);
        expect(check.isAnswered(SERIAL, "1.4.0")).toBe(true);

        check.release = { ...check.release!, version: "1.4.0" };
        expect(check.offer(SERIAL, "1.3.0"), "an older channel pointer is not a new question").toBeNull();
    });

    it("survives a reload", async () => {
        const storage = memoryLedger();
        (await checked(storage)).answer(SERIAL, "1.4.0");
        const reloaded = await checked(storage);
        expect(reloaded.offer(SERIAL, "1.3.0")).toBeNull();
        expect(reloaded.offer(OTHER_SERIAL, "1.3.0")).not.toBeNull();
    });

    it("drops the oldest answers past the cap", async () => {
        const storage = memoryLedger();
        const check = await checked(storage);
        check.answer(SERIAL, "0.0.1");
        for (let i = 0; i < LEDGER_CAP; i++) check.answer(SERIAL, `9.9.${i}`);
        const written = JSON.parse(storage.map.get("obcm.fwPromptAnswered")!) as string[];
        expect(written).toHaveLength(LEDGER_CAP);
        expect(written).not.toContain(`${SERIAL}@0.0.1`);
        // The entry aged out, but a newer answer still suppresses a rolled-back channel pointer.
        expect(check.isAnswered(SERIAL, "0.0.1")).toBe(true);
        expect(check.isAnswered(SERIAL, `9.9.${LEDGER_CAP - 1}`)).toBe(true);
    });

    it("ignores a ledger that is not the shape it wrote", async () => {
        for (const raw of ["not json", '{"a":1}', "[1,2,3]"]) {
            const check = await checked(memoryLedger({ "obcm.fwPromptAnswered": raw }));
            expect(check.offer(SERIAL, "1.3.0"), raw).not.toBeNull();
        }
    });

    it("keeps working when the store refuses to write", async () => {
        const check = await checked({ get: () => null, set: () => { throw new Error("quota"); } });
        expect(() => check.answer(SERIAL, "1.4.0")).not.toThrow();
        // The answer still holds for this page — it is only the remembering that failed.
        expect(check.offer(SERIAL, "1.3.0")).toBeNull();
    });

    it("ignores an answer it cannot scope to a device", async () => {
        const check = await checked(memoryLedger());
        check.answer(null, "1.4.0");
        expect(check.offer(SERIAL, "1.3.0")).not.toBeNull();
    });
});
