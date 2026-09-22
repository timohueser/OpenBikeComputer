/**
 * `npm run smoke` — one map onto the board over USB, one reboot, one proof.
 *
 * This file is the operator's half: arguments, the replacement plan it makes the operator agree to,
 * the two things `runSmoke` cannot do itself (the cable and the probe), and the record it leaves
 * behind. The run itself is `test-support/device-smoke/smoke.ts`, which is also what the Vitest
 * suite drives, so nothing about the sequence is written twice.
 *
 * The card is never formatted and nothing is removed. The only write is a `PUT` that replaces the
 * map object the firmware would select — the same replacement the builder performs when a rider
 * presses Send — and the run refuses to start until the operator has seen which object that is.
 */

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { ObjectKind } from "../../src/lib/usb/protocol";
import { loadSmokeFixture } from "../../test-support/device-smoke/fixture";
import { SmokeFailure, runSmoke } from "../../test-support/device-smoke/smoke";
import { connect } from "./link";
import { reboot } from "./board";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");
const DEFAULT_ELF = join(
    REPO_ROOT,
    "firmware/obc-fw-nrf54l/target/thumbv8m.main-none-eabihf/release/obc-fw-nrf54l",
);

const USAGE = `usage: npm run smoke -- --replace-map [OPTIONS]

  --replace-map        agree to replace the map object the device would select
  --elf <path>         the exact ELF of the running image (default: the release build)
  --probe <id>         VID:PID:SERIAL of the debug probe, when more than one is attached
  --serial <id>        the device's USB serial number, when more than one board is attached
  --out <path>         where to write the result (default: .artifacts/device-smoke/)
`;

interface Args {
    replaceMap: boolean;
    elf: string;
    probe?: string;
    serial?: string;
    out?: string;
}

function parseArgs(argv: readonly string[]): Args {
    const args: Args = { replaceMap: false, elf: DEFAULT_ELF };
    for (let at = 0; at < argv.length; at += 1) {
        const value = () => {
            const next = argv[at + 1];
            if (next === undefined) throw new Error(`${argv[at]} needs a value`);
            at += 1;
            return next;
        };
        switch (argv[at]) {
            case "--replace-map":
                args.replaceMap = true;
                break;
            case "--elf":
                args.elf = resolve(value());
                break;
            case "--probe":
                args.probe = value();
                break;
            case "--serial":
                args.serial = value();
                break;
            case "--out":
                args.out = resolve(value());
                break;
            case "-h":
            case "--help":
                process.stdout.write(USAGE);
                process.exit(0);
                break;
            default:
                throw new Error(`unknown argument ${argv[at]}\n\n${USAGE}`);
        }
    }
    return args;
}

/** Show the operator exactly which object the run will replace, and stop unless they agreed. */
async function statePlan(args: Args, fixtureName: string): Promise<void> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(new Error("no device answered in 20 s")), 20_000);
    const session = await connect(controller.signal, args.serial);
    try {
        const maps = await session.client.list({ kind: ObjectKind.MapShard, signal: controller.signal });
        const current = maps.entries.reduce<(typeof maps.entries)[number] | null>(
            (best, entry) => (!best || entry.objectId < best.objectId ? entry : best),
            null,
        );
        say(`store ${maps.storeId}, ${maps.entries.length} map object(s)`);
        say(
            current
                ? `${fixtureName} replaces object ${current.objectId} revision ${current.revision} ` +
                      `("${current.displayName}", ${current.payloadLength} B). Nothing else is written.`
                : `${fixtureName} creates the card's first map object. Nothing else is written.`,
        );
        if (!args.replaceMap) {
            throw new Error("Pass --replace-map to agree to that replacement.");
        }
    } finally {
        clearTimeout(timer);
        controller.abort();
        await session.release();
    }
}

function say(line: string): void {
    process.stderr.write(`smoke: ${line}\n`);
}

/** The repository state the run was made from, so the record names a build and not a machine. */
function toolCommit(): string {
    const head = execFileSync("git", ["rev-parse", "HEAD"], { cwd: REPO_ROOT, encoding: "utf8" }).trim();
    const dirty = execFileSync("git", ["status", "--porcelain"], { cwd: REPO_ROOT, encoding: "utf8" }).trim();
    return dirty ? `${head}-dirty` : head;
}

async function main(): Promise<number> {
    const args = parseArgs(process.argv.slice(2));
    const fixture = loadSmokeFixture(REPO_ROOT);
    say(`map ${fixture.name}, ${fixture.bytes.length} B, sha256 ${fixture.sha256}`);
    await statePlan(args, fixture.name);

    const started = new Date();
    let firmware = { elf: args.elf, sha256: "" };
    let probeLog = "";
    const report = await runSmoke({
        fixture,
        connect: (signal) => connect(signal, args.serial),
        reboot: async (signal) => {
            const restarted = await reboot({ repoRoot: REPO_ROOT, elf: args.elf, probe: args.probe }, signal);
            firmware = restarted.firmware;
            probeLog = restarted.log;
            return restarted.observation;
        },
        onPhase: (phase, note) => say(note ? `${phase}: ${note}` : phase),
    });

    const record = {
        startedAt: started.toISOString(),
        finishedAt: new Date().toISOString(),
        toolCommit: toolCommit(),
        firmware: { ...firmware, reported: report.device },
        card: { storeId: report.committed.storeId, preservedObjects: report.preservedObjects },
        fixture: {
            path: fixture.name,
            bytes: fixture.bytes.length,
            sha256: fixture.sha256,
            producerCommit: fixture.producerCommit,
        },
        committed: {
            storeId: report.committed.storeId,
            objectId: report.committed.objectId.toString(),
            revision: report.committed.revision.toString(),
            kind: report.committed.kind,
            payloadLength: report.committed.payloadLength.toString(),
            payloadCrc32: `0x${(report.committed.payloadCrc32 >>> 0).toString(16).padStart(8, "0")}`,
            displayName: report.committed.displayName,
        },
        boot: {
            objectId: report.boot.objectId.toString(),
            revision: report.boot.revision.toString(),
            payloadLength: report.boot.payloadLength.toString(),
            bbox: report.boot.bbox,
            terrainBytes: report.boot.terrainBytes,
        },
        phaseMs: report.phaseMs,
    };

    const out = args.out ?? join(REPO_ROOT, ".artifacts", "device-smoke", `${started.toISOString()}.json`);
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, `${JSON.stringify(record, null, 2)}\n`);
    writeFileSync(`${out}.rtt.log`, probeLog);
    say(`pass — ${out}`);
    return 0;
}

main().then(
    (code) => process.exit(code),
    (cause: unknown) => {
        if (cause instanceof SmokeFailure) {
            say(`FAILED in ${cause.phase} (${cause.reason}): ${cause.message}`);
        } else {
            say(`FAILED: ${cause instanceof Error ? cause.message : String(cause)}`);
        }
        process.exit(1);
    },
);
