//! **Verification**: is the nowcast actually better than what it replaces? (WXR9 #1251)
//!
//! The engine in [`crate::flow`] is only worth shipping if the frames it produces score better
//! against what actually happened than the two things they displace: **persistence** (the observed
//! field, frozen — what "radar nowcasting" degenerates to if the motion estimate is worthless) and
//! **the model** (HRRR, ICON-EU or the GFS floor — what the mosaic publishes at those offsets
//! today). #1251 asks for exactly that, and names it the deliverable that makes the rest arguable.
//!
//! This module is the scoring half. The data half already exists: an event pack
//! ([`crate::pack`]) carries `truth/`, a ladder of **observed** frames at +15 … +120 minutes past
//! its anchor, captured for this purpose and byte-verified in CI. `tests/nowcast_skill.rs` drives
//! the two together over the 2020-08-10 Midwest derecho.
//!
//! ## The metrics, and why these
//!
//! * **CSI** (critical success index, `hits / (hits + misses + false alarms)`) at a rain-rate
//!   threshold. It is the standard categorical score for precipitation and it deliberately ignores
//!   correct negatives, which on a continental raster are 90 % of the cells and would make every
//!   method look excellent.
//! * **FSS** (fractions skill score) at a neighbourhood radius. CSI is a point-by-point score and
//!   punishes a forecast that puts the right storm two cells from where it went as hard as one
//!   that misses it entirely — the double-penalty problem. FSS asks whether the *fraction* of wet
//!   cells in a neighbourhood matches, which is the question a rider deciding whether to shelter
//!   is actually asking.
//!
//! ## The honesty rule this scoring follows
//!
//! **No-data in the forecast counts as "no rain forecast".** The nowcast's characteristic blind
//! spot is the upwind edge: ground the observed field advected away from, with nothing behind it,
//! which [`crate::flow::advect`] leaves as [`precip4::INTENSITY_NODATA`] rather than as dry. Scoring
//! only where the nowcast has data would hide precisely the weakness that decides whether it should
//! outrank a model, so it does not: those cells are scored as misses where the truth is wet.
//! [`Scores::covered`] reports how much of the verified area the forecast actually answered for, so
//! the penalty is visible rather than merely suffered.
//!
//! Cells where the **truth** is no-data are excluded from every count. There is nothing to verify
//! against there, and including them would score a forecast against a radar's own coverage mask.

use obc_formats::precip4;

/// The intensity codes the scoring thresholds sit at.
///
/// Code 3 is `>= 0.25 mm/h` — "it is raining on you" — and code 6 is `>= 2.0 mm/h`, the rate at
/// which a rider starts thinking about shelter. Two thresholds rather than one because advection
/// skill decays much faster for the heavy, convective end than for the light, stratiform end, and a
/// single number would average that distinction away.
pub const LIGHT_RAIN: u8 = 3;
pub const MODERATE_RAIN: u8 = 6;

/// FSS neighbourhood half-width in cells. On the 1 km canonical lattice this is a 21 x 21 km box —
/// roughly the distance a rider covers in an hour, and the scale at which "will it rain on me" is a
/// meaningful question.
pub const FSS_RADIUS_CELLS: u32 = 10;

/// A 2x2 contingency table over the verifiable cells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Contingency {
    pub hits: u64,
    pub misses: u64,
    pub false_alarms: u64,
    pub correct_negatives: u64,
    /// Cells the forecast had no data for. A subset of `misses + correct_negatives`, counted
    /// separately so the blind-spot penalty is reportable.
    pub forecast_missing: u64,
}

impl Contingency {
    pub fn verified(&self) -> u64 {
        self.hits + self.misses + self.false_alarms + self.correct_negatives
    }

    /// Critical success index. `None` when nothing was observed and nothing forecast, where the
    /// score is undefined rather than perfect.
    pub fn csi(&self) -> Option<f64> {
        let denominator = self.hits + self.misses + self.false_alarms;
        (denominator > 0).then(|| self.hits as f64 / denominator as f64)
    }

    pub fn pod(&self) -> Option<f64> {
        let denominator = self.hits + self.misses;
        (denominator > 0).then(|| self.hits as f64 / denominator as f64)
    }

    pub fn far(&self) -> Option<f64> {
        let denominator = self.hits + self.false_alarms;
        (denominator > 0).then(|| self.false_alarms as f64 / denominator as f64)
    }

    /// Fraction of the verified area the forecast actually answered for.
    pub fn covered(&self) -> f64 {
        let verified = self.verified();
        if verified == 0 {
            return 0.0;
        }
        1.0 - self.forecast_missing as f64 / verified as f64
    }
}

/// Score `forecast` against `truth` at one intensity threshold.
///
/// Both are intensity-code rasters of the same shape. See the module comment for the two rules:
/// truth no-data is excluded, forecast no-data is "no rain forecast".
pub fn contingency(forecast: &[u8], truth: &[u8], threshold: u8) -> Contingency {
    assert_eq!(forecast.len(), truth.len(), "contingency: the two rasters must be the same shape");
    let mut table = Contingency::default();
    for (&predicted, &observed) in forecast.iter().zip(truth) {
        if observed == precip4::INTENSITY_NODATA {
            continue;
        }
        let missing = predicted == precip4::INTENSITY_NODATA;
        let predicted_wet = !missing && predicted >= threshold;
        let observed_wet = observed >= threshold;
        if missing {
            table.forecast_missing += 1;
        }
        match (predicted_wet, observed_wet) {
            (true, true) => table.hits += 1,
            (true, false) => table.false_alarms += 1,
            (false, true) => table.misses += 1,
            (false, false) => table.correct_negatives += 1,
        }
    }
    table
}

/// Fractions skill score at `threshold` over a `2 * radius + 1` square neighbourhood.
///
/// `1.0` is perfect, `0.0` is no skill at all. Computed the standard way — binary fields, box-mean
/// fractions, `1 - MSE / MSE_reference` — over a summed-area table, so a 786 k-cell frame with a
/// 21 x 21 neighbourhood costs two passes rather than 441.
///
/// Truth no-data cells are excluded from the score but still contribute their (zero) fraction to
/// the neighbourhoods around them, exactly as a dry cell would. That is the conservative reading:
/// an unscanned cell is not evidence of rain.
pub fn fss(forecast: &[u8], truth: &[u8], width: u32, height: u32, threshold: u8, radius: u32) -> Option<f64> {
    let count = width as usize * height as usize;
    assert_eq!(forecast.len(), count, "fss: the forecast does not match its dimensions");
    assert_eq!(truth.len(), count, "fss: the truth does not match its dimensions");
    if count == 0 {
        return None;
    }
    let binary = |cells: &[u8]| -> Vec<f64> {
        cells
            .iter()
            .map(|&code| if code != precip4::INTENSITY_NODATA && code >= threshold { 1.0 } else { 0.0 })
            .collect()
    };
    let forecast_fractions = box_mean(&binary(forecast), width, height, radius);
    let truth_fractions = box_mean(&binary(truth), width, height, radius);

    let (mut error, mut reference, mut scored) = (0.0f64, 0.0f64, 0u64);
    for index in 0..count {
        if truth[index] == precip4::INTENSITY_NODATA {
            continue;
        }
        let (predicted, observed) = (forecast_fractions[index], truth_fractions[index]);
        error += (predicted - observed).powi(2);
        reference += predicted * predicted + observed * observed;
        scored += 1;
    }
    if scored == 0 {
        return None;
    }
    if reference == 0.0 {
        // Nothing wet anywhere in either field, at this threshold, within a neighbourhood of
        // anything verifiable. Undefined rather than perfect: there was no event to have skill at.
        return None;
    }
    Some(1.0 - error / reference)
}

/// Mean of a `2 * radius + 1` square neighbourhood at every cell, edge-truncated (the box is
/// clipped at the raster edge and divided by the cells it actually covered).
fn box_mean(values: &[f64], width: u32, height: u32, radius: u32) -> Vec<f64> {
    let (w, h) = (width as usize, height as usize);
    let radius = radius as isize;
    // Summed-area table with a zero row and column, so a box sum is four lookups.
    let mut area = vec![0.0f64; (w + 1) * (h + 1)];
    for row in 0..h {
        let mut running = 0.0;
        for col in 0..w {
            running += values[row * w + col];
            area[(row + 1) * (w + 1) + col + 1] = area[row * (w + 1) + col + 1] + running;
        }
    }
    let sum = |x0: isize, y0: isize, x1: isize, y1: isize| {
        let (x0, y0) = (x0.max(0) as usize, y0.max(0) as usize);
        let (x1, y1) = ((x1.min(w as isize)) as usize, (y1.min(h as isize)) as usize);
        let value =
            area[y1 * (w + 1) + x1] - area[y0 * (w + 1) + x1] - area[y1 * (w + 1) + x0] + area[y0 * (w + 1) + x0];
        let cells = ((x1 - x0) * (y1 - y0)) as f64;
        if cells > 0.0 {
            value / cells
        } else {
            0.0
        }
    };
    let mut out = vec![0.0f64; w * h];
    for row in 0..h as isize {
        for col in 0..w as isize {
            out[row as usize * w + col as usize] = sum(col - radius, row - radius, col + radius + 1, row + radius + 1);
        }
    }
    out
}

/// One method's scores at one lead time.
#[derive(Debug, Clone, Copy)]
pub struct Scores {
    pub light: Contingency,
    pub moderate: Contingency,
    pub fss_light: Option<f64>,
    pub fss_moderate: Option<f64>,
}

impl Scores {
    pub fn of(forecast: &[u8], truth: &[u8], width: u32, height: u32) -> Self {
        Self {
            light: contingency(forecast, truth, LIGHT_RAIN),
            moderate: contingency(forecast, truth, MODERATE_RAIN),
            fss_light: fss(forecast, truth, width, height, LIGHT_RAIN, FSS_RADIUS_CELLS),
            fss_moderate: fss(forecast, truth, width, height, MODERATE_RAIN, FSS_RADIUS_CELLS),
        }
    }

    /// `CSI(>=0.25) CSI(>=2.0) FSS(>=0.25) FSS(>=2.0) coverage`, formatted for a report table.
    pub fn row(&self) -> String {
        let show = |value: Option<f64>| value.map(|value| format!("{value:.3}")).unwrap_or_else(|| "  -  ".into());
        format!(
            "{}  {}  {}  {}  {:.3}",
            show(self.light.csi()),
            show(self.moderate.csi()),
            show(self.fss_light),
            show(self.fss_moderate),
            self.light.covered()
        )
    }

    pub fn covered(&self) -> f64 {
        self.light.covered()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_perfect_forecast_scores_one_and_a_useless_one_scores_zero() {
        let truth = vec![0, 0, 5, 8, 0, 6, 0, 0];
        let perfect = truth.clone();
        assert_eq!(contingency(&perfect, &truth, LIGHT_RAIN).csi(), Some(1.0));
        let dry = vec![0u8; truth.len()];
        assert_eq!(contingency(&dry, &truth, LIGHT_RAIN).csi(), Some(0.0));
    }

    /// The two honesty rules, as arithmetic: truth no-data is not scored, forecast no-data is a
    /// forecast of no rain and is counted as such.
    #[test]
    fn missing_data_is_scored_the_way_the_module_says() {
        let truth = vec![5, precip4::INTENSITY_NODATA, 0, 7];
        let forecast = vec![precip4::INTENSITY_NODATA, 9, 0, 7];
        let table = contingency(&forecast, &truth, LIGHT_RAIN);
        assert_eq!(table.verified(), 3, "the truth's no-data cell is not verified");
        assert_eq!(table.hits, 1, "cell 3");
        assert_eq!(table.misses, 1, "cell 0: the forecast had no data where it rained");
        assert_eq!(table.correct_negatives, 1, "cell 2");
        assert_eq!(table.false_alarms, 0);
        assert_eq!(table.forecast_missing, 1);
        assert!((table.covered() - 2.0 / 3.0).abs() < 1e-9);
    }

    /// FSS forgives a near miss that CSI destroys — the double-penalty problem, which is the whole
    /// reason both scores are reported.
    #[test]
    fn fss_credits_a_displaced_forecast_that_csi_gives_nothing_for() {
        let (width, height) = (64u32, 64u32);
        let mut truth = vec![0u8; (width * height) as usize];
        let mut shifted = vec![0u8; (width * height) as usize];
        for row in 28..36u32 {
            for col in 28..36u32 {
                truth[(row * width + col) as usize] = 8;
                // The same block, three cells east: no overlap at all.
                shifted[(row * width + col + 8) as usize] = 8;
            }
        }
        assert_eq!(contingency(&shifted, &truth, LIGHT_RAIN).csi(), Some(0.0), "CSI sees a total miss");
        let score = fss(&shifted, &truth, width, height, LIGHT_RAIN, FSS_RADIUS_CELLS).expect("an event to score");
        assert!(score > 0.5, "FSS must credit a near miss, got {score}");
        // …and still rank a perfect forecast above it.
        assert!(fss(&truth, &truth, width, height, LIGHT_RAIN, FSS_RADIUS_CELLS).expect("perfect") > score);
    }

    #[test]
    fn a_neighbourhood_mean_is_the_mean_of_the_neighbourhood() {
        let values = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let mean = box_mean(&values, 3, 3, 1);
        // Every cell's 3x3 box is clipped to the raster, so the centre sees all nine.
        assert!((mean[4] - 3.0 / 9.0).abs() < 1e-12);
        // The north-west corner sees a 2x2 box holding two of the diagonal's ones.
        assert!((mean[0] - 2.0 / 4.0).abs() < 1e-12);
    }
}
