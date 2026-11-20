//! Numbers typed into plain text fields of the settings. The limits of the sync
//! thresholds and the rules between them live in `sppy_sync::SyncParams`.

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

/// A whole number of at least `min`, and at most `max` when there is a limit.
pub fn whole(text: &str, unit: &str, min: u64, max: Option<u64>) -> Result<u64, String> {
    let text = strip_unit(text, unit);
    if text.is_empty() {
        return Err("Enter a number".into());
    }
    let v: u64 = text.parse().map_err(|_| "Enter a whole number".to_owned())?;
    match max {
        Some(max) if !(min..=max).contains(&v) => Err(format!("From {min} to {max}")),
        None if v < min => Err(format!("At least {min}")),
        _ => Ok(v),
    }
}

/// Percent (a dot or a comma for fractions) as a fraction from `min` to `max`:
/// `8` → 0.08.
pub fn percent(text: &str, (min, max): (f64, f64)) -> Result<f64, String> {
    let text = strip_unit(text, "%").replace(',', ".");
    if text.is_empty() {
        return Err("Enter a number".into());
    }
    let v: f64 = text.parse().map_err(|_| "Enter a number".to_owned())?;
    let v = v / 100.0;
    if !v.is_finite() || !(min..=max).contains(&v) {
        return Err(format!("From {} to {}", percent_text(min), percent_text(max)));
    }
    Ok(v)
}

/// A fraction as the percent shown in a field: 0.08 → `8`, 0.025 → `2.5`.
pub fn percent_text(fraction: f64) -> String {
    format!("{}", (fraction * 1000.0).round() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_numbers_with_or_without_the_unit() {
        assert_eq!(whole(" 100 ", "ms", 10, None), Ok(100));
        assert_eq!(whole("100 ms", "ms", 10, None), Ok(100));
        assert_eq!(whole("100ms", "ms", 10, None), Ok(100));
        assert_eq!(whole("", "ms", 10, None), Err("Enter a number".into()));
        assert_eq!(whole("1.5", "ms", 10, None), Err("Enter a whole number".into()));
        assert_eq!(whole("-5", "ms", 10, None), Err("Enter a whole number".into()));
        assert_eq!(whole("5", "ms", 10, None), Err("At least 10".into()));
        assert_eq!(whole("999999999", "ms", 10, None), Ok(999_999_999));
        assert_eq!(whole("0", "", 1, Some(65_535)), Err("From 1 to 65535".into()));
        assert_eq!(whole("65536", "", 1, Some(65_535)), Err("From 1 to 65535".into()));
        assert_eq!(whole("8080", "", 1, Some(65_535)), Ok(8080));
    }

    #[test]
    fn percents_become_fractions() {
        assert_eq!(percent("8", (0.01, 0.5)), Ok(0.08));
        assert_eq!(percent("2,5 %", (0.0, 0.2)), Ok(0.025));
        assert_eq!(percent("2.5%", (0.0, 0.2)), Ok(0.025));
        assert_eq!(percent("0.08", (0.01, 0.5)), Err("From 1 to 50".into()));
        assert_eq!(percent("NaN", (0.0, 0.2)), Err("From 0 to 20".into()));
        assert_eq!(percent("fast", (0.0, 0.2)), Err("Enter a number".into()));
        assert_eq!(percent_text(0.08), "8");
        assert_eq!(percent_text(0.025), "2.5");
        assert_eq!(percent_text(0.0), "0");
    }
}
