// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { URL as NodeURL } from "node:url";
import { flushSync, mount, unmount } from "svelte";
import { expect, it, vi } from "vitest";
import { CatalogClient } from "../../lib/catalog/client";
import { CoverageStore } from "../../lib/coverage/store.svelte";
import SkinStep from "./SkinStep.svelte";

const ROOT = readFileSync(new NodeURL("../../../../../host/obc-pack/schema/catalog.example.json", import.meta.url), "utf8");

// The pictures of the skins need the wasm core; a skin with no picture stays in the list.
vi.mock("../../lib/skin/preview", () => ({ renderSkinPreviewFrames: async () => ({}) }));

const offline = (async () => new Response("offline", { status: 503 })) as unknown as typeof fetch;
const button = (name: string, within: ParentNode = document) =>
    [...within.querySelectorAll("button")].find((candidate) => candidate.textContent?.trim() === name);

it("asks in a window before it deletes a style of the rider", () => {
    const client = CatalogClient.fromBody(ROOT, "https://maps.example.org/catalog/catalog.json", { fetchImpl: offline });
    const store = new CoverageStore(client, ROOT, { getItem: () => null, setItem: () => {} });
    const mine = { ...store.lightSkin, id: "custom-mine", name: "Mine" };
    store.customSkinRecords = [{ skin: mine, based_on: "default" }];
    store.lightSkinId = mine.id;
    const shown = mount(SkinStep, { target: document.body, props: { store, plain: true } });
    flushSync();
    const question = () => document.querySelector("dialog");

    button("Delete")!.click();
    flushSync();
    expect(question()!.textContent).toContain("Delete “Mine”?");
    button("Cancel", question()!)!.click();
    expect(store.lightSkin.id).toBe(mine.id);

    button("Delete")!.click();
    flushSync();
    button("Delete", question()!)!.click();
    expect(store.customSkinRecords).toEqual([]);
    expect(store.lightSkin.id).toBe("default");
    unmount(shown);
});
