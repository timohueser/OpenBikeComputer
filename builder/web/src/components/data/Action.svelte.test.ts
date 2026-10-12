// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, expect, it, vi } from "vitest";
import Action from "./Action.svelte";

afterEach(() => {
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

function serverAnswers(body: unknown) {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(body)));
    vi.stubGlobal("fetch", fetch);
    return fetch;
}

it("posts exactly the args that it shows, and never shows --json", async () => {
    const fetch = serverAnswers({ exit: 0, output: { run: "r1" } });
    const ondone = vi.fn();
    const args = ["update", "local", "--region", "Black Forest", "--yes"];
    const action = mount(Action, { target: document.body, props: { args, label: "Update local", ondone } });
    try {
        expect(document.querySelector("code")?.textContent).toBe("obc data update local --region 'Black Forest' --yes");
        document.querySelector("button")?.click();
        await vi.waitFor(() => expect(ondone).toHaveBeenCalledWith({ run: "r1" }));
        expect(fetch).toHaveBeenCalledOnce();
        const [url, init] = fetch.mock.calls[0];
        expect(url).toBe("/api/cli");
        expect(JSON.parse(init.body)).toEqual({ args });
        expect(document.body.textContent).not.toContain("--json");
    } finally {
        await unmount(action);
    }
});

it("shows the error of a failed command with its fix", async () => {
    serverAnswers({
        exit: 2,
        output: { error: { code: "busy", message: "hetzner-bake-1 updates prod", fix: "Watch it with `obc data runs`." } },
    });
    const ondone = vi.fn();
    const action = mount(Action, { target: document.body, props: { args: ["rollback", "prod", "--yes"], label: "Roll back", ondone } });
    try {
        document.querySelector("button")?.click();
        const failure = await vi.waitFor(() => {
            const shown = document.querySelector("[role=alert]");
            if (!shown) throw new Error("no failure shown yet");
            return shown;
        });
        expect(failure.textContent).toContain("hetzner-bake-1 updates prod");
        expect(failure.textContent).toContain("Fix · Watch it with obc data runs.");
        expect(failure.querySelector("code")?.textContent).toBe("obc data runs");
        expect(ondone).not.toHaveBeenCalled();
    } finally {
        await unmount(action);
    }
});
