// The hosts are imported directly, not through `$host`, because the point is
// to check both in one run.

import { describe, expect, it } from "vitest";
import * as dev from "./dev";
import * as web from "./web";

const HOSTS = { web, dev };

describe.each(Object.entries(HOSTS))("%s host", (name, host) => {
    it("names itself", () => {
        expect(host.platform.name).toBe(name);
    });
});

describe("the hosts as a set", () => {
    it("loads the style editor only on the dev host", () => {
        // The web host consumes published cells; only the dev host loads the
        // maintainer schema editor.
        expect(web.platform.styleEditor).toBeNull();
        expect(dev.platform.styleEditor).not.toBeNull();
    });
});
