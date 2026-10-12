// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import example from "../../../test-support/data/review.json";
import status from "../../../test-support/data/status.json";
import type { Review as ReviewShape, Status } from "../../lib/data/shapes";
import Review from "./Review.svelte";

const review = example as ReviewShape;
const layers = (status as Status).targets.find(({ target }) => target === "local")!.layers!;
const props = { name: "local", kind: "dir" as const, flags: [], machine: "laptop", locked: null, layers };

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
    location.hash = "";
});

/** The server answers the review of `local` with the example, and a start with a run. */
function server() {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        const output = args.includes("--dry-run") ? review : { run: "2026-10-12-120000", review: review.id };
        return new Response(JSON.stringify({ exit: 0, output }));
    });
    vi.stubGlobal("fetch", fetch);
    return () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args as string[]);
}

const shown = () => [...document.querySelectorAll("code")].map((code) => code.textContent);
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent === label)!;

it("starts exactly the shown args with --expect, and only after the format change is accepted", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Review ${review.id}: update local on laptop`));
        expect(posted()).toEqual([["update", "local", "--dry-run"]]);
        expect(shown()).toContain(`obc data update local --expect ${review.id} --accept-format-change`);
        expect(button("Start on laptop").disabled).toBe(true);

        document.querySelector<HTMLInputElement>(".gate input")!.click();
        await vi.waitFor(() => expect(button("Start on laptop").disabled).toBe(false));
        button("Start on laptop").click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-12-120000"));
        expect(posted().slice(1)).toEqual([review.args]);
    } finally {
        await unmount(screen);
    }
});

it("an edit becomes a flag of the next review", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props: { ...props, flags: [["--add", "sun"]] } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Review ${review.id}`));
        const snow = [...document.querySelectorAll("label")].find((label) => label.textContent?.trim() === "snow")!;
        snow.querySelector("input")!.click();
        const next = ["update", "local", "--add", "sun", "--add", "snow", "--dry-run"];
        await vi.waitFor(() => expect(posted().at(-1)).toEqual(next));
        expect(shown()).toContain("obc data update local --add sun --add snow --dry-run");
    } finally {
        await unmount(screen);
    }
});

it("on an R2 target, Start says that it publishes to riders and posts the args with --yes", async () => {
    const posted = server();
    const screen = mount(Review, { target: document.body, props: { ...props, name: "prod", kind: "r2" } });
    try {
        await vi.waitFor(() => expect(document.body.textContent).toContain(`Publishes review ${review.id} to prod, which serves riders.`));
        document.querySelector<HTMLInputElement>(".gate input")!.click();
        await vi.waitFor(() => expect(button("Start on laptop").disabled).toBe(false));
        button("Start on laptop").click();
        await vi.waitFor(() => expect(posted().slice(1)).toEqual([[...review.args, "--yes"]]));
    } finally {
        await unmount(screen);
    }
});
