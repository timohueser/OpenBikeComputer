/**
 * Device Object Protocol v3 — the browser/desktop client's wire codec.
 *
 * Written from the normative tables in `specs/Device_Object_Protocol_v3.md`,
 * `specs/Device_Object_Registries_v2.md` and `specs/Device_Object_System_v2.md`, and pinned against
 * the shared vectors in `specs/vectors/device-object-v2/` by `vectors.test.ts`. It is one of the
 * three independent implementations DOS1 requires; that independence is the point, so nothing here
 * is generated from — or derived by reading — the Rust or Swift codec.
 *
 * Nothing in this module throws on malformed input: every entry point either returns a decoded
 * value or a `DosError` carrying the specification's own category and detail.
 */

export * from "./bytes";
export * from "./capabilities";
export * from "./errorBody";
export * from "./frame";
export * from "./ids";
export * from "./intent";
export * from "./messages";
export * from "./metadata";
export * from "./registry";
export * from "./result";
export * from "./stream";
