const KEY = "obc-theme";
type Theme = "light" | "dark";
export const appearance = $state({ dark: false });
let preference: Theme | null = null;

function savedTheme(): Theme | null {
    try {
        const saved = localStorage.getItem(KEY);
        return saved === "light" || saved === "dark" ? saved : null;
    } catch {
        return null;
    }
}

function apply(dark: boolean) {
    appearance.dark = dark;
    document.documentElement.dataset.theme = dark ? "dark" : "light";
}

export function startTheme(): () => void {
    const system = matchMedia("(prefers-color-scheme: dark)");
    preference = savedTheme();
    const sync = () => apply(preference ? preference === "dark" : system.matches);
    const onStorage = (event: StorageEvent) => {
        if (event.key !== KEY && event.key !== null) return;
        preference = savedTheme();
        sync();
    };
    sync();
    system.addEventListener("change", sync);
    window.addEventListener("storage", onStorage);
    return () => {
        system.removeEventListener("change", sync);
        window.removeEventListener("storage", onStorage);
    };
}

export function toggleTheme() {
    preference = appearance.dark ? "light" : "dark";
    apply(preference === "dark");
    try { localStorage.setItem(KEY, preference); } catch { /* The toggle still works without storage. */ }
}
