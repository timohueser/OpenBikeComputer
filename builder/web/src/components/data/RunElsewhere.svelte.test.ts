// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it } from "vitest";
import status from "../../../test-support/data/status.json";
import { elsewhere, type Status } from "../../lib/data/shapes";
import RunElsewhere from "./RunElsewhere.svelte";

afterEach(() => document.body.replaceChildren());

it("shows the phase of a run on another machine and why it waits", async () => {
    const [{ target, holder }] = elsewhere(status as Status);
    const screen = mount(RunElsewhere, { target: document.body, props: { target, holder, ended: false } });
    try {
        const phases = document.querySelector(".phases")?.textContent?.replace(/\s+/g, " ");
        expect(phases).toContain("✓ upload ok");
        expect(phases).toContain("◐ router-ready running : the planner release is starting");
    } finally {
        await unmount(screen);
    }
});
