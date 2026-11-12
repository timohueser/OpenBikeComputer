const KEY = "obc-theme";
/** `system` follows the computer: then `obc-theme` has no value. */
export type Look = "system" | "light" | "dark";
// The page's inline script sets the theme before the first paint, so the store starts from it.
export const appearance = $state<{ dark: boolean; look: Look }>({
    dark: typeof document !== "undefined" && document.documentElement.dataset.theme === "dark",
    look: "system",
});
let system: MediaQueryList | undefined;

function savedLook(): Look {
    try {
        const saved = localStorage.getItem(KEY);
        return saved === "light" || saved === "dark" ? saved : "system";
    } catch {
        return "system";
    }
}

function sync() {
    appearance.dark = appearance.look === "system" ? !!system?.matches : appearance.look === "dark";
    document.documentElement.dataset.theme = appearance.dark ? "dark" : "light";
}

export function startTheme(): () => void {
    system = matchMedia("(prefers-color-scheme: dark)");
    appearance.look = savedLook();
    const onStorage = (event: StorageEvent) => {
        if (event.key !== KEY && event.key !== null) return;
        appearance.look = savedLook();
        sync();
    };
    sync();
    system.addEventListener("change", sync);
    window.addEventListener("storage", onStorage);
    return () => {
        system?.removeEventListener("change", sync);
        window.removeEventListener("storage", onStorage);
    };
}

export function setLook(look: Look) {
    appearance.look = look;
    sync();
    try {
        if (look === "system") localStorage.removeItem(KEY);
        else localStorage.setItem(KEY, look);
    } catch { /* The look still applies to this page without storage. */ }
}

export function toggleTheme() {
    setLook(appearance.dark ? "light" : "dark");
}
