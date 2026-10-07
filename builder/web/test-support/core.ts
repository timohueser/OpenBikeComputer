import { readFileSync } from "node:fs";
import { initCore } from "../src/lib/core/bridge";

await initCore(readFileSync("src/lib/core/pkg/obc_builder_bridge_bg.wasm"));
