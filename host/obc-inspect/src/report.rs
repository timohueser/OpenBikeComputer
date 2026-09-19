//! One report tree, rendered two ways.
//!
//! Every printer below builds a [`Report`]. The text output and the `--json` output are two
//! renderings of that one tree, so a field can never appear in one and be missing from the other.
//! Insertion order is the print order in both, which is why the JSON is written here rather than
//! through a `serde_json::Map` — that map sorts its keys, and a section table sorted by key is not
//! a section table. Leaves are still `serde_json::Value`, so every string is escaped by serde.

use std::fmt::Write as _;

use serde_json::Value;

/// What one row of a report holds.
enum Field {
    /// A scalar, or an array of scalars.
    Leaf(Value),
    /// A named sub-report.
    Group(Report),
    /// A table: rows of the same shape.
    List(Vec<Report>),
}

/// An ordered set of named fields.
#[derive(Default)]
pub struct Report(Vec<(String, Field)>);

impl Report {
    pub fn new() -> Report {
        Report(Vec::new())
    }

    pub fn put(&mut self, key: &str, value: impl Into<Value>) -> &mut Report {
        self.0.push((key.to_string(), Field::Leaf(value.into())));
        self
    }

    pub fn group(&mut self, key: &str, report: Report) -> &mut Report {
        self.0.push((key.to_string(), Field::Group(report)));
        self
    }

    pub fn list(&mut self, key: &str, rows: Vec<Report>) -> &mut Report {
        self.0.push((key.to_string(), Field::List(rows)));
        self
    }

    /// Append another report's fields, keeping both orders. The caller's own fields come first.
    pub fn extend(&mut self, other: Report) -> &mut Report {
        self.0.extend(other.0);
        self
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        self.write_text(&mut out, 0);
        out
    }

    pub fn json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out, 0);
        out.push('\n');
        out
    }

    fn write_text(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        for (key, field) in &self.0 {
            match field {
                Field::Leaf(value) => {
                    let _ = writeln!(out, "{pad}{key}: {}", scalar(value));
                }
                Field::Group(report) => {
                    let _ = writeln!(out, "{pad}{key}:");
                    report.write_text(out, indent + 1);
                }
                Field::List(rows) if rows.is_empty() => {
                    let _ = writeln!(out, "{pad}{key}: (none)");
                }
                Field::List(rows) => {
                    let _ = writeln!(out, "{pad}{key}:");
                    for row in rows {
                        // The dash marks the row; its remaining fields line up under the first one.
                        let mut body = String::new();
                        row.write_text(&mut body, indent + 2);
                        let mut lines = body.lines();
                        match lines.next() {
                            Some(first) => {
                                let _ = writeln!(out, "{pad}  - {}", first.trim_start());
                                for line in lines {
                                    let _ = writeln!(out, "{line}");
                                }
                            }
                            None => {
                                let _ = writeln!(out, "{pad}  -");
                            }
                        }
                    }
                }
            }
        }
    }

    fn write_json(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent + 1);
        if self.0.is_empty() {
            out.push_str("{}");
            return;
        }
        out.push_str("{\n");
        for (at, (key, field)) in self.0.iter().enumerate() {
            let _ = write!(out, "{pad}{}: ", Value::String(key.clone()));
            match field {
                Field::Leaf(value) => out.push_str(&value.to_string()),
                Field::Group(report) => report.write_json(out, indent + 1),
                Field::List(rows) if rows.is_empty() => out.push_str("[]"),
                Field::List(rows) => {
                    out.push_str("[\n");
                    for (at, row) in rows.iter().enumerate() {
                        out.push_str(&"  ".repeat(indent + 2));
                        row.write_json(out, indent + 2);
                        out.push_str(if at + 1 == rows.len() { "\n" } else { ",\n" });
                    }
                    let _ = write!(out, "{pad}]");
                }
            }
            out.push_str(if at + 1 == self.0.len() { "\n" } else { ",\n" });
        }
        let _ = write!(out, "{}}}", "  ".repeat(indent));
    }
}

/// A leaf as one line of text. Strings print bare; an array prints comma-separated.
fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(scalar).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// Microdegrees as degrees, the unit every coordinate in this repository is stored in.
pub fn degrees(udeg: i64) -> Value {
    Value::String(format!("{:.6}", udeg as f64 / 1e6))
}

/// A byte count with its human size beside it, so a section length reads at a glance.
pub fn bytes(count: u64) -> Value {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")];
    for (size, name) in UNITS {
        if count >= size {
            return Value::String(format!("{count} ({:.1} {name})", count as f64 / size as f64));
        }
    }
    Value::String(count.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Report {
        let mut row = Report::new();
        row.put("level", 0).put("chunks", 4);
        let mut nested = Report::new();
        nested.put("version", 18);
        let mut report = Report::new();
        report.put("format", "obcm").group("header", nested).list("lods", vec![row]).list("peaks", vec![]);
        report
    }

    #[test]
    fn text_prints_the_fields_in_declaration_order() {
        assert_eq!(
            sample().text(),
            "format: obcm\nheader:\n  version: 18\nlods:\n  - level: 0\n    chunks: 4\npeaks: (none)\n"
        );
    }

    #[test]
    fn json_parses_and_keeps_that_same_order() {
        let text = sample().json();
        let parsed: Value = serde_json::from_str(&text).expect("the report is valid JSON");
        assert_eq!(parsed["header"]["version"], 18);
        assert_eq!(parsed["lods"][0]["chunks"], 4);
        assert_eq!(parsed["peaks"], Value::Array(Vec::new()));
        assert!(text.find("\"format\"") < text.find("\"header\""), "declaration order survives");
    }
}
