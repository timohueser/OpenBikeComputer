import { initCore } from "../src/lib/core/bridge";

void initCore().then(() => import("./main-app")).catch((cause: unknown) => {
    document.getElementById("app")!.textContent = `The builder core could not be loaded. Reload the page to retry. ${String(cause)}`;
});
