//! "Natural" ordering for names shown to people: `Season 2` before `Season 10`.

use std::cmp::Ordering;

enum Chunk<'a> {
    Digits(&'a str),
    Text(String),
}

fn chunks(s: &str) -> Vec<Chunk<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_digits = None;
    for (i, c) in s.char_indices() {
        let d = c.is_ascii_digit();
        match in_digits {
            Some(prev) if prev != d => {
                out.push(chunk(&s[start..i], prev));
                start = i;
            }
            _ => {}
        }
        in_digits = Some(d);
    }
    if let Some(d) = in_digits {
        out.push(chunk(&s[start..], d));
    }
    out
}

fn chunk(s: &str, digits: bool) -> Chunk<'_> {
    if digits { Chunk::Digits(s) } else { Chunk::Text(s.to_lowercase()) }
}

/// Compare digit runs by value (any length), text case-insensitively; ties fall back to the
/// plain string order so the result is total and stable.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(&cb) {
        let ord = match (x, y) {
            (Chunk::Digits(x), Chunk::Digits(y)) => {
                let (tx, ty) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty)).then_with(|| y.len().cmp(&x.len()))
            }
            (Chunk::Digits(_), Chunk::Text(_)) => Ordering::Less,
            (Chunk::Text(_), Chunk::Digits(_)) => Ordering::Greater,
            (Chunk::Text(x), Chunk::Text(y)) => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    ca.len().cmp(&cb.len()).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_sort_by_value() {
        assert_eq!(
            sorted(vec!["Season 10", "Season 2", "Season 1", "Season 9"]),
            vec!["Season 1", "Season 2", "Season 9", "Season 10"]
        );
        assert_eq!(
            sorted(vec!["S01E10.mkv", "S01E2.mkv", "S01E01.mkv"]),
            vec!["S01E01.mkv", "S01E2.mkv", "S01E10.mkv"]
        );
    }

    #[test]
    fn case_insensitive_and_stable() {
        assert_eq!(sorted(vec!["b", "A", "a"]), vec!["A", "a", "b"]);
        assert_eq!(natural_cmp("x", "x"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn leading_zeros_and_long_numbers() {
        assert_eq!(sorted(vec!["e007", "e7", "e06"]), vec!["e06", "e007", "e7"]);
        assert_eq!(
            sorted(vec!["n99999999999999999999999", "n2"]),
            vec!["n2", "n99999999999999999999999"]
        );
    }

    #[test]
    fn unicode_letters() {
        assert_eq!(
            sorted(vec!["Серия 10", "Серия 2", "серия 1"]),
            vec!["серия 1", "Серия 2", "Серия 10"]
        );
    }
}
