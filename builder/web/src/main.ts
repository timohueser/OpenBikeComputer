import { mount } from "svelte";
import "leaflet/dist/leaflet.css";
import "./styles/app.css";
import { initCore } from "./lib/core/bridge";

void initCore().then(async () => {
    const { default: App } = await import("./App.svelte");
    mount(App, { target: document.getElementById("app")! });
}).catch((cause: unknown) => {
    document.getElementById("app")!.textContent = `The builder core could not be loaded. Reload the page to retry. ${String(cause)}`;
});
