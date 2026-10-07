//! Live: the region of `data/env/live.toml`, each product with its live release, the day of its
//! apply and its state, the optional layers, and what needs attention. It shows what `status`
//! writes.

use ratatui::layout::Rect;
use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::{bytes, row_text, state_style, widths, App, Effect, Overlay, Switch};
use crate::cli::status_cli::{AttentionKind, ProductStatus};
use crate::sources::State;

/// A row of Live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LiveRow {
    Region,
    Schedule,
    Product(usize),
    /// An optional layer, by name.
    Layer(String),
    Attention(usize),
}

/// Where `enter` on a row of NEEDS ATTENTION goes.
pub(super) enum Fix {
    /// The source on Sources.
    Source(usize),
    /// Store, whose clean moves an old cache in.
    Store,
    /// Plan, whose apply repairs drift and removes leftovers.
    Plan,
}

impl App {
    pub(super) fn live_rows(&self) -> Vec<LiveRow> {
        let mut rows = vec![LiveRow::Region];
        let Some(status) = &self.status else { return rows };
        if self.schedule.state.is_some() {
            rows.push(LiveRow::Schedule);
        }
        rows.extend((0..status.products.len()).map(LiveRow::Product));
        rows.extend(self.optional().into_iter().map(LiveRow::Layer));
        rows.extend((0..status.attention.len()).map(LiveRow::Attention));
        rows
    }

    /// The optional layers of every product.
    fn optional(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let offered = self.status.iter().flat_map(|status| &status.products).flat_map(|product| &product.optional);
        for name in offered {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    fn layer_on(&self, name: &str) -> bool {
        self.env.as_ref().is_some_and(|env| env.layers.iter().any(|layer| layer == name))
    }

    pub(super) fn fix(&self) -> Option<Fix> {
        let Some(LiveRow::Attention(i)) = self.live_rows().get(self.row).cloned() else { return None };
        let attention = &self.status.as_ref()?.attention[i];
        match attention.kind {
            AttentionKind::Stale | AttentionKind::Blocked => {
                self.sources.iter().position(|row| row.source.id == attention.about).map(Fix::Source)
            }
            AttentionKind::OldCache => Some(Fix::Store),
            AttentionKind::Drift | AttentionKind::Leftovers => Some(Fix::Plan),
            AttentionKind::Unreachable => None,
        }
    }

    /// `space`: switch the selected optional layer of Live, or take or leave the selected move of
    /// Plan.
    pub(super) fn toggle(&mut self) -> Effect {
        if self.overlay == Some(Overlay::Plan) {
            return self.plan.as_mut().map_or(Effect::None, |view| Effect::Select(view.toggle()));
        }
        match self.live_rows().get(self.row) {
            Some(LiveRow::Layer(name)) => {
                let switch = if self.layer_on(name) { Switch::Off } else { Switch::On };
                Effect::Layer(name.clone(), switch)
            }
            _ => Effect::None,
        }
    }

    pub(super) fn draw_live(&mut self, frame: &mut Frame, area: Rect) {
        let header = self.status.as_ref().map_or("LIVE".into(), |status| format!("LIVE from {}", status.from));
        let rows = self.live_rows();
        let products = self.status.as_ref().map_or(&[][..], |status| &status.products[..]);
        let attention = self.status.as_ref().map_or(&[][..], |status| &status.attention[..]);
        let mut table = vec![["PRODUCT", "RELEASE", "APPLIED", "SIZE"].map(String::from).to_vec()];
        table.extend(products.iter().map(|product| {
            let release = product.release.as_ref().map_or("nothing live".into(), |id| format!("release {}", &id[..8]));
            let applied = product.applied.as_deref().and_then(|time| time.get(..10)).unwrap_or("—");
            vec![product.product.clone(), release, applied.into(), product.bytes.map_or("—".into(), bytes)]
        }));
        let product_widths = widths(&table);
        let needs: Vec<Vec<String>> =
            attention.iter().map(|a| vec![a.kind.text().into(), a.about.clone(), a.reason.clone()]).collect();
        let needs_widths = widths(&needs);
        let layer_width = self.optional().iter().map(|name| name.chars().count()).max().unwrap_or(0);

        let (mut lines, mut at) = (Vec::new(), Vec::new());
        let gap = |lines: &mut Vec<Line>, title: String| lines.extend([Line::default(), Line::from(title).dim()]);
        for (i, row) in rows.iter().enumerate() {
            match row {
                LiveRow::Product(0) => {
                    gap(&mut lines, row_text(&[&table[0][..], &["STATE".into()]].concat(), &product_widths))
                }
                LiveRow::Layer(_) if !matches!(rows[i - 1], LiveRow::Layer(_)) => {
                    gap(&mut lines, "OPTIONAL LAYERS".into())
                }
                LiveRow::Attention(0) => gap(&mut lines, "NEEDS ATTENTION".into()),
                _ => {}
            }
            at.push(lines.len());
            let line = match row {
                LiveRow::Region => {
                    let region = self.env.as_ref().map_or("—".into(), |env| env.region.clone());
                    Line::from(vec![Span::from("region  ").dim(), Span::from(region)])
                }
                LiveRow::Schedule => Line::from(format!("automation  {} · s details", self.schedule.summary())),
                LiveRow::Product(p) => {
                    let cells = row_text(&[&table[p + 1][..], &[String::new()]].concat(), &product_widths);
                    Line::from(vec![Span::from(cells), product_state(&products[*p])])
                }
                LiveRow::Layer(name) => {
                    let mark = if self.layer_on(name) { "[x]" } else { "[ ]" };
                    let mut spans = vec![Span::from(format!("{mark} {name:<layer_width$}  "))];
                    let layer = products.iter().find_map(|product| {
                        let layers = product.layers.as_ref()?;
                        layers.iter().find(|layer| layer.layer == format!("{}/{name}", product.product))
                    });
                    spans.extend(layer.map(|layer| Span::styled(layer.state.to_string(), state_style(layer.state))));
                    Line::from(spans)
                }
                LiveRow::Attention(a) => {
                    let (kind, rest) = needs[*a].split_first().expect("a row has cells");
                    let color = match attention[*a].kind {
                        AttentionKind::Blocked | AttentionKind::Drift | AttentionKind::Unreachable => Color::Red,
                        _ => Color::Yellow,
                    };
                    let kind = Span::styled(format!("{kind:<w$}  ", w = needs_widths[0]), color);
                    Line::from(vec![kind, Span::from(row_text(rest, &needs_widths[1..]))])
                }
            };
            lines.push(if i == self.row { line.reversed() } else { line });
        }
        if self.status.is_some() && attention.is_empty() {
            gap(&mut lines, "NEEDS ATTENTION".into());
            lines.push(Line::from("nothing").green());
        }
        let end = at[self.row.min(at.len() - 1)];
        self.draw_lines(frame, area, Line::from(header), lines, &at, end);
    }
}

/// The state of a product: the first state of its layers in the order of `State`.
fn product_state(product: &ProductStatus) -> Span<'static> {
    let Some(layers) = &product.layers else { return Span::styled("unknown", Color::Red) };
    let state = layers.iter().map(|layer| layer.state).min().unwrap_or(State::Ok);
    Span::styled(state.to_string(), state_style(state))
}
