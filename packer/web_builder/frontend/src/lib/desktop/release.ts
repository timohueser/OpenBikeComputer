// What the desktop page offers to download. D3 (#908) builds, signs (or
// decides not to) and publishes the installers; until it does, `RELEASE` is
// null and the page says there are no builds yet rather than linking at files
// that do not exist. Inventing a URL now would cost a visitor a 404 and cost
// us the one thing the page is for.

export interface DesktopDownload {
    readonly os: "macOS" | "Windows" | "Linux";
    /** Only where one OS ships more than one build (`Apple silicon`, `x86-64`). */
    readonly arch?: string;
    readonly filename: string;
    readonly url: string;
    /** Bytes, for the size shown next to the button. */
    readonly size: number;
    /** Lowercase hex, 64 chars. Shown in full: a checksum you have to expand to
     *  read is a checksum nobody checks. */
    readonly sha256: string;
}

export interface DesktopRelease {
    readonly version: string;
    /** ISO date, `YYYY-MM-DD`. */
    readonly date: string;
    readonly downloads: readonly DesktopDownload[];
    /**
     * Set only if D3 ships unsigned builds, and written as **instructions**:
     * what to click, in what order, in the imperative. "Right-click the app and
     * choose Open, then confirm" — not "unfortunately macOS will warn you that
     * …". The page renders it under a *First run* heading, so it needs no
     * preamble explaining that something is about to be awkward.
     */
    readonly installNote?: string;
}

/** Filled in by D3 (#908). `src/lib/desktop/release.test.ts` holds it to its
 *  shape from that commit onwards. */
export const RELEASE: DesktopRelease | null = null;
