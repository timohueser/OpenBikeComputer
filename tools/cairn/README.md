# cairn

`cairn check` compares the code with the system docs in `docs/design/`. It uses only the Python
standard library and `cargo metadata`.

```sh
obc cairn check            # print the report
obc cairn check --update   # remove gone entries from the baseline, and lower sizes that shrank
python3 -m unittest discover -s tools/cairn/tests
```

## Files

| File | Holds |
| --- | --- |
| `docs/design/<system>.md` | the header of one system or shell: `kind`, `area`, `purpose`, `owns`, `depends_on`, `test_depends_on`, `status` |
| `.cairn/config.json` | the source file types, the adapters, the runtime table for R2, the tooling folder for R4 |
| `.cairn/baseline.json` | the known violations, by ID, with a size |
| `adapters/cargo.py` | the Rust crates and their path dependencies, from every configured Cargo root |

In `owns`, `**` matches any characters and `*` matches any characters except `/`.

## Rules

| Rule | The check reports |
| --- | --- |
| Ownership | a tracked source file that no doc owns, or that two docs own. A system beats a shell: a shell owns only the files that no system owns. |
| Dependency | a production edge between two systems that `depends_on` does not list, or a test edge that neither `depends_on` nor `test_depends_on` lists. The system that owns a crate's entry file (`src/lib.rs` or `src/main.rs`) owns all of its edges. |
| Cycle | each production edge inside a cycle between systems. A cycle is a violation even when the docs declare its edges. |
| R1 | a Rust crate that holds files of a system other than the owner of its entry file |
| R2 | a system with code under two roots of the same runtime in the `runtimes` table. The longest matching root applies. The check ignores a file under no root. |
| R3 | each shell's own lines: the lines of the files that no system owns. Each shell's doc sets its target size in Phase 2. |
| R4 | a file under `tooling.path` whose owner is outside `tooling.area` |

## Ratchet

A violation that is not in the baseline is new. An R3 or R4 violation whose size (lines or files)
is above its baseline size is also new. A new violation fails only when it touches a system whose
doc has `status: approved`. Until then, the check reports and exits with 0.

`--update` never adds an entry and never raises a size. `--accept` writes the first baseline and
refuses when the file exists.

`firmware/tools/check_dependencies.py` stays. Its crate groups do not follow the system cut, so it
keeps the production direction rules until cairn enforces.
