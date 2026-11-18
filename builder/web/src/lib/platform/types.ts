/** Host services used by the shared app. */
export interface Platform {
    /** Resolved catalog URL and its raw root document. */
    catalog(options?: { refresh?: boolean }): Promise<{ url: string; body: string }>;
    readonly catalogFetch: typeof fetch;

    readonly siteNav: { readonly home: string; readonly docs: string; readonly blog: string; readonly github: string };
}
