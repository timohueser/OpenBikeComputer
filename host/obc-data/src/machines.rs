//! `data/machines.toml`: the header colour of the page of each machine, and the SSH host that
//! `obc data open` tunnels to. A machine that the file does not name is a laptop.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// The header colour of a machine that `data/machines.toml` does not name.
pub const LAPTOP: &str = "#262624";

/// The page shows this grey in its header until it knows the machine.
const LOADING: &str = "#6e6c66";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    /// `#rrggbb`, dark enough for white text.
    pub colour: String,
    /// The SSH host or alias of the machine; its name without it.
    #[serde(default)]
    pub ssh: Option<String>,
}

/// The machines of `data/machines.toml`; none without the file.
pub fn load(root: &Path) -> Result<BTreeMap<String, Machine>, String> {
    let path = root.join("data/machines.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("data/machines.toml: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn parse(text: &str) -> Result<BTreeMap<String, Machine>, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// The header colour of the machine `name`. A file or a colour that is not valid gives the
/// charcoal of a laptop and a warning, so that it never stops a command.
pub fn colour(root: &Path, name: &str) -> String {
    let colour = load(root).and_then(|mut machines| match machines.remove(name) {
        Some(machine) => check_colour(&machine.colour)
            .map(|()| machine.colour)
            .map_err(|e| format!("data/machines.toml: machine `{name}`: {e}")),
        None => Ok(LAPTOP.to_string()),
    });
    colour.unwrap_or_else(|e| {
        eprintln!("obc data: {e}; the page header is charcoal");
        LAPTOP.to_string()
    })
}

/// The SSH host of the machine `name`.
pub fn ssh(root: &Path, name: &str) -> Result<String, String> {
    Ok(load(root)?.remove(name).and_then(|machine| machine.ssh).unwrap_or_else(|| name.to_string()))
}

/// White text on `colour` has the contrast of WCAG AA, 4.5:1, so the colour has a relative
/// luminance of at most 0.1833.
fn check_colour(colour: &str) -> Result<(), String> {
    let hex = colour.strip_prefix('#').filter(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
    let hex = hex.ok_or_else(|| format!("colour `{colour}` is not #rrggbb"))?;
    if colour.eq_ignore_ascii_case(LOADING) {
        return Err(format!("colour `{colour}` is the grey of a page that loads"));
    }
    let channel = |i: usize| {
        let value = f64::from(u8::from_str_radix(&hex[i..i + 2], 16).expect("two hex digits")) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
    if 1.05 / (luminance + 0.05) < 4.5 {
        return Err(format!("colour `{colour}` is too light for the white text of the header"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_machines_have_header_colours_for_white_text() {
        for (name, machine) in parse(include_str!("../../../data/machines.toml")).unwrap() {
            check_colour(&machine.colour).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let bake = parse("[bake]\ncolour = \"#4a2a6a\"\nssh = \"root@bake\"\n").unwrap().remove("bake").unwrap();
        assert_eq!(bake, Machine { colour: "#4a2a6a".into(), ssh: Some("root@bake".into()) });
        assert!(check_colour(LAPTOP).is_ok());
        for (colour, refusal) in [
            ("#ffd9a8", "too light"),
            ("#777777", "too light"),
            ("#6E6C66", "grey of a page that loads"),
            ("purple", "not #rrggbb"),
            ("#4a2a6", "not #rrggbb"),
        ] {
            let error = check_colour(colour).unwrap_err();
            assert!(error.contains(refusal), "{colour}: {error}");
        }
    }
}
