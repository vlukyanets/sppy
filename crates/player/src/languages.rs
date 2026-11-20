//! Preferred track languages as the players take them.

/// Track language list for the players (mpv's `alang`/`slang`, VLC's
/// `audio-language`/`sub-language`): the languages as given plus their other ISO 639 form, since
/// files name languages either way (`en` / `eng`) and older mpv versions match them
/// literally.
pub(crate) fn language_codes(languages: &[String]) -> String {
    const PAIRS: &[(&str, &[&str])] = &[
        ("en", &["eng"]),
        ("ru", &["rus"]),
        ("uk", &["ukr"]),
        ("de", &["deu", "ger"]),
        ("fr", &["fra", "fre"]),
        ("es", &["spa"]),
        ("it", &["ita"]),
        ("pt", &["por"]),
        ("pl", &["pol"]),
        ("ja", &["jpn"]),
        ("ko", &["kor"]),
        ("zh", &["zho", "chi"]),
    ];
    let mut out: Vec<String> = Vec::new();
    let mut push = |code: &str| {
        if !code.is_empty() && !out.iter().any(|c| c == code) {
            out.push(code.to_owned());
        }
    };
    for lang in languages {
        let lang = lang.trim().to_ascii_lowercase();
        push(&lang);
        for (two, three) in PAIRS {
            if lang == *two {
                three.iter().for_each(|c| push(c));
            } else if three.contains(&lang.as_str()) {
                push(two);
                three.iter().for_each(|c| push(c));
            }
        }
    }
    out.join(",")
}

#[cfg(test)]
mod tests {
    use super::language_codes as codes;

    #[test]
    fn languages_get_both_iso_forms_in_order() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(codes(&l(&["ru", "en"])), "ru,rus,en,eng");
        assert_eq!(codes(&l(&["ENG", " ger "])), "eng,en,ger,de,deu");
        assert_eq!(codes(&l(&["en", "eng", "xx"])), "en,eng,xx");
        assert_eq!(codes(&[]), "");
    }
}
