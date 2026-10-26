// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import example from "../../../test-support/data/deploy.json";
import dry from "../../../test-support/data/deploy-plan.json";
import listed from "../../../test-support/data/legacy.json";
import { CliQuery } from "../../lib/data/query.svelte";
import type { Deployed, RunList } from "../../lib/data/shapes";
import OldData from "./OldData.svelte";
import Services from "./Services.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

/** The server answers each command of `answers`, by its args. */
function serverAnswers(answers: Record<string, unknown>) {
    const fetch = vi.fn(async (_url: string, init: { body: string }) => {
        const args: string[] = JSON.parse(init.body).args;
        return new Response(JSON.stringify({ exit: 0, output: answers[JSON.stringify(args)] }));
    });
    vi.stubGlobal("fetch", fetch);
    return fetch;
}

const text = () => document.body.textContent ?? "";
const button = (label: string) => [...document.querySelectorAll("button")].find((button) => button.textContent?.trim() === label);
const posted = (fetch: ReturnType<typeof serverAnswers>) => fetch.mock.calls.map(([, init]) => JSON.parse(init.body).args);

it("shows what each deploy lacks with its fix, and deploys only after the dry run and a confirm", async () => {
    const deployed = example as Deployed;
    // The VPS lets this machine in; the tiles Worker still lacks its token.
    const ready = { ...deployed, services: { ...deployed.services, checks: [deployed.services.checks[0]] } };
    const fetch = serverAnswers({
        '["deploy"]': ready,
        [JSON.stringify(["deploy", "services", "--dry-run"])]: dry,
        [JSON.stringify([...dry.args, "--yes"])]: { run: "2026-10-26-090049" },
    });
    const query = new CliQuery<Deployed>(["deploy"]);
    await query.refresh();
    const runs = new CliQuery<RunList>(["runs"]);
    const screen = mount(Services, { target: document.body, props: { deployed: query, runs } });
    try {
        expect(text()).toContain("✗ Not deployed: GET https://releases.openbikecomputer.com/planner-api/version: HTTP 404");
        expect(text()).toContain("✓ Serves the map tiles of each release · commit 35d0 (this checkout)");
        expect(text()).toContain("the tiles Worker needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID");
        expect(document.querySelector("input[name=CLOUDFLARE_API_TOKEN]")).not.toBeNull();
        const deploys = [...document.querySelectorAll("button")].filter((button) => button.textContent === "Deploy 35d0…");
        expect(deploys.map((button) => button.disabled)).toEqual([false, true]);

        deploys[0].click();
        await vi.waitFor(() => expect(text()).toContain("Deploy commit 35d0?"));
        expect(text()).toContain('tools/vps/deploy-services.sh "$OBC_VPS_SSH" 35d0b9f1');
        button("Deploy now")!.click();
        await vi.waitFor(() => expect(location.hash).toBe("#/runs/2026-10-26-090049"));
        expect(posted(fetch).slice(1)).toEqual([["deploy", "services", "--dry-run"], [...dry.args, "--yes"]]);
    } finally {
        await unmount(screen);
    }
});

it("a refused login gives the line to add on the VPS, with a copy button", async () => {
    serverAnswers({ '["deploy"]': example });
    const query = new CliQuery<Deployed>(["deploy"]);
    await query.refresh();
    const screen = mount(Services, { target: document.body, props: { deployed: query, runs: new CliQuery<RunList>(["runs"]) } });
    try {
        expect(text()).toContain("login as root · root@vps.example: Permission denied (publickey).");
        const line = "mkdir -p /root/.ssh && echo 'ssh-ed25519 AAAAexample owner@bake' >> /root/.ssh/authorized_keys";
        const code = [...document.querySelectorAll("code")].find((code) => code.textContent === line)!;
        expect(code.nextElementSibling?.textContent).toBe("Copy");
        expect(text()).toContain("Fix login as root first.");
    } finally {
        await unmount(screen);
    }
});

it("Old data counts the legacy keys and deletes exactly the list that it showed", async () => {
    const fetch = serverAnswers({ '["r2","legacy","prod"]': listed, [JSON.stringify(["r2", "legacy", "prod", "--delete", "--expect", listed.digest, "--reason", "Legacy publish path: the target serves releases of obc data", "--yes"])]: { bucket: "obc-maps", objects: [] } });
    const publish = (example as Deployed).publish;
    const screen = mount(OldData, { target: document.body, props: { name: "prod", serving: true, publish } });
    try {
        await vi.waitFor(() => expect(text()).toContain("3 keys, 1.0 GB of the legacy publish path"));
        button("Delete old data…")!.click();
        await vi.waitFor(() => expect(text()).toContain("This cannot be undone."));
        button("Delete now")!.click();
        await vi.waitFor(() => expect(posted(fetch)).toHaveLength(3));
        expect(posted(fetch)[1]).toEqual(["r2", "legacy", "prod", "--delete", "--expect", listed.digest, "--reason", "Legacy publish path: the target serves releases of obc data", "--yes"]);
    } finally {
        await unmount(screen);
    }
});
