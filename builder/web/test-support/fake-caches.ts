// Cache Storage in memory for the device cache's tests: what a put keeps survives an engine, as on a device.
export class FakeCaches {
    caches = new Map<string, Map<string, { body: ArrayBuffer; headers: Headers }>>();

    async open(name: string) {
        const entries = this.caches.get(name) ?? new Map<string, { body: ArrayBuffer; headers: Headers }>();
        this.caches.set(name, entries);
        return {
            match: async (url: string) => {
                const entry = entries.get(url);
                return entry && new Response(entry.body.slice(0), { headers: entry.headers });
            },
            put: async (url: string, response: Response) => void entries.set(url, { body: await response.arrayBuffer(), headers: response.headers }),
            delete: async (url: string) => entries.delete(url),
            keys: async () => [...entries.keys()].map(url => ({ url })),
        };
    }

    async keys() {
        return [...this.caches.keys()];
    }

    async delete(name: string) {
        return this.caches.delete(name);
    }
}
