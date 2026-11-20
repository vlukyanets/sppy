//! Numbers typed into plain text fields of the settings: parsing, limits and the rules
//! that tie the sync thresholds together.

use sppy_client_core::config::SyncParams;

/// What is typed into one number field while the settings are open.
pub struct Draft {
    pub text: String,
    /// Why the text is not a valid value; the setting keeps its last valid value meanwhile.
    pub error: Option<String>,
}

/// The text without the unit the field shows next to it anyway (`100 ms`, `8 %`).
fn strip_unit<'a>(text: &'a str, unit: &str) -> &'a str {
    let text = text.trim();
    text.strip_suffix(unit).unwrap_or(text).trim_end()
}

/// A whole number from `min` to `max`.
pub fn whole(text: &str, unit: &str, min: u64, max: u64) -> Result<u64, String> {
    let text = strip_unit(text, unit);
    if text.is_empty() {
        return Err("Enter a number".into());
    }
    let v: u64 = text.parse().map_err(|_| "Enter a whole number".to_owned())?;
    if !(min..=max).contains(&v) {
        return Err(format!("From {min} to {max}"));
    }
    Ok(v)
}

/// Percent from `min` to `max` (a dot or a comma for fractions) as a fraction: `8` → 0.08.
pub fn percent(text: &str, min: f64, max: f64) -> Result<f64, String> {
    let text = strip_unit(text, "%").replace(',', ".");
    if text.is_empty() {
        return Err("Enter a number".into());
    }
    let v: f64 = text.parse().map_err(|_| "Enter a number".to_owned())?;
    if !v.is_finite() || !(min..=max).contains(&v) {
        return Err(format!("From {min} to {max}"));
    }
    Ok(v / 100.0)
}

/// A fraction as the percent shown in a field: 0.08 → `8`, 0.025 → `2.5`.
pub fn percent_text(fraction: f64) -> String {
    format!("{}", (fraction * 1000.0).round() / 10.0)
}

/// What is wrong with thresholds that are each within their limits but not together.
pub fn sync_problems(p: &SyncParams) -> Vec<&'static str> {
    let mut out = Vec::new();
    if p.speed_exit_ms >= p.dead_band_ms {
        out.push("Speed correction exit must be below the dead band.");
    }
    if p.seek_threshold_ms <= p.dead_band_ms {
        out.push("Seek threshold must be above the dead band.");
    }
    if p.min_speed_correction > p.max_speed_correction {
        out.push("Min speed correction must not be above the max.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_numbers_with_or_without_the_unit() {
        assert_eq!(whole(" 100 ", "ms", 10, 5000), Ok(100));
        assert_eq!(whole("100 ms", "ms", 10, 5000), Ok(100));
        assert_eq!(whole("100ms", "ms", 10, 5000), Ok(100));
        assert_eq!(whole("", "ms", 10, 5000), Err("Enter a number".into()));
        assert_eq!(whole("1.5", "ms", 10, 5000), Err("Enter a whole number".into()));
        assert_eq!(whole("-5", "ms", 10, 5000), Err("Enter a whole number".into()));
        assert_eq!(whole("5", "ms", 10, 5000), Err("From 10 to 5000".into()));
        assert_eq!(whole("5001", "ms", 10, 5000), Err("From 10 to 5000".into()));
    }

    #[test]
    fn percents_become_fractions() {
        assert_eq!(percent("8", 1.0, 50.0), Ok(0.08));
        assert_eq!(percent("2,5 %", 0.0, 20.0), Ok(0.025));
        assert_eq!(percent("2.5%", 0.0, 20.0), Ok(0.025));
        assert_eq!(percent("0.08", 1.0, 50.0), Err("From 1 to 50".into()));
        assert_eq!(percent("NaN", 0.0, 20.0), Err("From 0 to 20".into()));
        assert_eq!(percent("fast", 0.0, 20.0), Err("Enter a number".into()));
        assert_eq!(percent_text(0.08), "8");
        assert_eq!(percent_text(0.025), "2.5");
        assert_eq!(percent_text(0.0), "0");
    }

    #[test]
    fn defaults_are_consistent_and_conflicts_are_named() {
        assert!(sync_problems(&SyncParams::default()).is_empty());
        let p = SyncParams {
            dead_band_ms: 2000,
            min_speed_correction: 0.1,
            max_speed_correction: 0.05,
            ..SyncParams::default()
        };
        assert_eq!(
            sync_problems(&p),
            [
                "Seek threshold must be above the dead band.",
                "Min speed correction must not be above the max."
            ]
        );
        let p = SyncParams { speed_exit_ms: 100, ..SyncParams::default() };
        assert_eq!(sync_problems(&p), ["Speed correction exit must be below the dead band."]);
    }
}
