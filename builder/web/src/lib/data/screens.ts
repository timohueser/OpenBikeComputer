// Hash routes, so that the page works behind any SSH tunnel port with no server rewrites.

export const SCREENS = [
    { id: "overview", label: "Overview", hash: "#/" },
    { id: "prod", label: "prod", hash: "#/prod" },
    { id: "local", label: "local", hash: "#/local" },
    { id: "sources", label: "Sources", hash: "#/sources" },
    { id: "runs", label: "Runs", hash: "#/runs" },
    { id: "machine", label: "Machine", hash: "#/machine" },
] as const;

export type Screen = (typeof SCREENS)[number];

/** The screen of a hash such as `#/runs/ID`: its first segment names the screen. */
export function screenAt(hash: string): Screen {
    const first = hash.replace(/^#\/?/, "").split("/")[0];
    return SCREENS.find((screen) => screen.id === first) ?? SCREENS[0];
}
