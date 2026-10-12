import { expect, it } from "vitest";
import example from "../../../test-support/data/status.json";
import { ended, type Status } from "./shapes";

it("a run elsewhere ends when its lock is free or another run holds it, not when the lock cannot be read", () => {
    const status = example as Status;
    const prod = status.targets.find(({ target }) => target === "prod")!;
    const run = prod.lock!.run;
    const changed = (lock: Partial<Status["targets"][number]>): Status => ({ ...status, targets: [{ ...prod, ...lock }] });
    expect(ended(status, "prod", run)).toBe(false);
    expect(ended(changed({ lock: null }), "prod", run)).toBe(true);
    expect(ended(changed({ lock: { ...prod.lock!, run: "2026-10-12-120000" } }), "prod", run)).toBe(true);
    expect(ended(changed({ lock: null, lock_error: "lock.json: timed out" }), "prod", run)).toBe(false);
    expect(ended(null, "prod", run)).toBe(false);
});
