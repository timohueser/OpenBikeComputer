//! Host render benchmark harness plus a pixel-hash and read-counter golden gate.
//!
//! Renders a fixed 8-scene matrix through the real pipeline — `obcm-testkit`'s deterministic
//! fixture, `SliceSource`, `MapTables`/`MapCache`/`Reader`, `RenderScratch::render_timed`, the
//! device-resolution [`obc_display::Framebuffer565`] — and prints per-stage timings (min of 10 after a warm-up),
//! the [`obc_render::RenderStats`] counters, and an FNV-1a 64 hash of the frame's pixels.
//!
//! Two jobs, one binary. As a benchmark, the timings are the before-and-after numbers an
//! optimization lands with: printed, never gated, because shared CI runners are noisy. As a
//! tripwire, `--check` compares both the frame hashes and the map read path's per-case read
//! counters against the committed `golden.txt` and fails on any drift; both are deterministic,
//! from a seeded fixture, integer and `libm` math, and a fixed cache policy. Pixels catch a
//! rendering change; the counters catch a cache change that halves the hit rate while every pixel
//! stays identical. A pure refactor must touch neither; an intentional change regenerates the file
//! with `--write-golden` in the same PR.
//!
//! `--check` and `--write-golden` cover two matrices against one file: the 8 render scenes and the
//! 9 route-corridor snapshot cases, the latter under `corridor/` names.
//!
//! Modes: default (print the table), `--repeat <N>` (repeat the whole matrix and report
//! min/median/max), `--write-golden <file>`, `--check <file>` (exit 1 on mismatch), `--corridor`
//! (print the corridor matrix alone), and `--map <path> --mpp <f> --heading <deg>`, a manual escape
//! hatch to run one scene against a real local `.obcm` — never in CI, because real maps are not
//! byte-stable fixtures.

mod cli;
mod corridor;
mod golden;
mod scenes;
#[cfg(test)]
mod tests;

use std::{process::ExitCode, time::Instant};

use cli::{parse_args, Mode};
use corridor::{print_corridor_table, run_corridor_matrix, CorridorResult};
use golden::{check_golden, golden_lines};
use scenes::{print_repeat_table, print_table, run_matrix, run_scene, SceneResult, StdClock};

/// Run and print everything the golden file gates — the scene matrix and the corridor matrix — so
/// `--check` and `--write-golden` measure and report exactly the same thing.
fn run_gated_matrices() -> (Vec<SceneResult>, Vec<CorridorResult>) {
    let scenes = run_matrix();
    print_table(&scenes);
    let (corridor, route) = run_corridor_matrix();
    println!();
    print_corridor_table(&corridor, &route);
    (scenes, corridor)
}

fn main() -> ExitCode {
    let mode = match parse_args() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("obc-bench: {e}");
            eprintln!(
                "usage: obc-bench [--repeat <odd-N> | --write-golden <file> | --check <file> | --corridor | --map <path> [--mpp <f>] [--heading <deg>]]"
            );
            return ExitCode::FAILURE;
        }
    };

    match mode {
        Mode::Table => print_table(&run_matrix()),
        Mode::Corridor => {
            let (results, route) = run_corridor_matrix();
            print_corridor_table(&results, &route);
        }
        Mode::Repeat(n) => print_repeat_table(n),
        Mode::WriteGolden(path) => {
            let (scenes, corridor) = run_gated_matrices();
            if let Err(e) = std::fs::write(&path, golden_lines(&scenes, &corridor)) {
                eprintln!("obc-bench: writing {path}: {e}");
                return ExitCode::FAILURE;
            }
            println!("wrote {path}");
        }
        Mode::Check(path) => {
            let golden = match std::fs::read_to_string(&path) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("obc-bench: reading {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let (scenes, corridor) = run_gated_matrices();
            if !check_golden(&scenes, &corridor, &golden) {
                eprintln!(
                    "pixels or read counters drifted from {path} — intentional change? regenerate with \
                     --write-golden and state the reason in the same PR"
                );
                return ExitCode::FAILURE;
            }
            println!("all {} golden records match {path}", scenes.len() + corridor.len());
        }
        // Manual escape hatch: one scene over a real local `.obcm`. No hash bookkeeping, no
        // saturation assert — real maps aren't fixtures.
        Mode::Custom { map, mpp, heading } => {
            let bytes = match std::fs::read(&map) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("obc-bench: reading {map}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let clock = StdClock(Instant::now());
            print_table(&[run_scene(&bytes, "custom", mpp, heading, true, &clock)]);
        }
    }
    ExitCode::SUCCESS
}
