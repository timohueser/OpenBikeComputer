import { core } from "./bridge";

/** Rust owns the incremental hash. The adapter keeps the existing chaining API. */
export class Sha256 {
    private readonly hash = new (core().IncrementalSha256)();

    update(bytes: Uint8Array): this { this.hash.update(bytes); return this; }
    digest(): Uint8Array { return this.hash.digest(); }
    hex(): string { return Array.from(this.digest(), (b) => b.toString(16).padStart(2, "0")).join(""); }
    static hex(bytes: Uint8Array): string { return new Sha256().update(bytes).hex(); }
}
