/// What the hand-parsed CLI asked for. No CLI framework — five flags, parsed by hand.
pub(super) enum Mode {
    Table,
    Repeat(usize),
    WriteGolden(String),
    Check(String),
    Custom {
        map: String,
        mpp: f32,
        heading: f32,
    },
    /// The route-corridor snapshot cost matrix — SD reads + host time, not pixels.
    Corridor,
}

pub(super) fn parse_args() -> Result<Mode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut write, mut check, mut map, mut repeat) = (None, None, None, None);
    let mut corridor = false;
    let (mut mpp, mut heading) = (4.0f32, 0.0f32);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match a.as_str() {
            "--write-golden" => write = Some(val("--write-golden")?),
            "--check" => check = Some(val("--check")?),
            "--repeat" => {
                let n: usize = val("--repeat")?.parse().map_err(|e| format!("--repeat: {e}"))?;
                if n == 0 || n.is_multiple_of(2) {
                    return Err("--repeat must be a positive odd number (so the median is unambiguous)".into());
                }
                repeat = Some(n);
            }
            "--corridor" => corridor = true,
            "--map" => map = Some(val("--map")?),
            "--mpp" => mpp = val("--mpp")?.parse().map_err(|e| format!("--mpp: {e}"))?,
            "--heading" => heading = val("--heading")?.parse().map_err(|e| format!("--heading: {e}"))?,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(match (write, check, map, repeat, corridor) {
        (Some(f), None, None, None, false) => Mode::WriteGolden(f),
        (None, Some(f), None, None, false) => Mode::Check(f),
        (None, None, Some(map), None, false) => Mode::Custom { map, mpp, heading },
        (None, None, None, Some(n), false) => Mode::Repeat(n),
        (None, None, None, None, true) => Mode::Corridor,
        (None, None, None, None, false) => Mode::Table,
        _ => return Err("pick one of --repeat / --write-golden / --check / --corridor / --map".into()),
    })
}
