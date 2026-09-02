//! JAV code extraction from file names.
//!
//! Ported from JavBoss `internal/util/jav_util.go` (MIT-licensed reference in
//! `.tmp-javboss/`). Accepts common formats such as `IPX-633`, `ipx633`,
//! `ipx633_ch` and `ipx-714c`, plus uncensored-style codes (`FC2-...`,
//! pure-numeric `012345-678`).

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

fn code_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)([a-z]{2,6})[-_ ]?(\d{2,5})([a-z]{0,2})").expect("regex"))
}

fn alpha_numeric_uncensored_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(^|[^a-z0-9])([a-z]+)(?:\s*([-_ ])\s*)?(\d{2,})([^a-z0-9]|$)")
            .expect("regex")
    })
}

fn mixed_prefix_uncensored_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(^|[^a-z0-9])([a-z0-9]*[a-z][a-z0-9]*\d[a-z0-9]*[a-z][a-z0-9]*)[-_ ](\d{2,})([^a-z0-9]|$)",
        )
        .expect("regex")
    })
}

fn mixed_prefix_censored_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(^|[^a-z0-9])([a-z][a-z0-9]{1,5})[-_ ](\d{2,5})([a-z]{0,2})([^a-z0-9]|$)")
            .expect("regex")
    })
}

fn pure_numeric_uncensored_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(^|[^0-9])(\d{4,}[-_]\d{2,})([^0-9]|$)").expect("regex"))
}

fn explicit_short_code_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(^|[^a-z0-9])([a-z]{2,6})[-_ ](\d{2})([a-z]{0,2})([^a-z0-9]|$)")
            .expect("regex")
    })
}

/// FC2 marketplace titles (`FC2-PPV-1234567`, `FC2-1234567`, `FC2PPV-1234567`).
/// Extension over JavBoss, whose generic patterns only yield noise such as
/// `PPV-12345` for these names.
fn fc2_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(^|[^a-z0-9])FC2[-_ ]?(PPV[-_ ]?)?(\d{3,10})([^0-9]|$)").expect("regex")
    })
}

fn extract_fc2_codes(base: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for caps in fc2_re().captures_iter(base) {
        let number = caps.get(3).map(|m| m.as_str().trim()).unwrap_or_default();
        if number.is_empty() {
            continue;
        }
        // JavBus/Avmoo list FC2 works under the full "FC2-PPV-xxxxxxx" form,
        // so the PPV variant is the stronger candidate.
        if caps.get(2).is_some() {
            append_unique(&mut out, &mut seen, format!("FC2-PPV-{number}"));
        }
        append_unique(&mut out, &mut seen, format!("FC2-{number}"));
    }
    out
}

/// Extract candidate JAV codes from a file name or path. Results are
/// upper-cased, de-duplicated and ordered: censored-style codes first, then
/// uncensored-style candidates.
pub fn extract_codes_from_name(name: &str) -> Vec<String> {
    let base = file_stem(name);
    // FC2 marketplace names short-circuit the generic patterns: once the FC2
    // shape is recognized, candidates like "PPV-12345" are pure noise that
    // would waste source lookups.
    let fc2 = extract_fc2_codes(&base);
    if !fc2.is_empty() {
        return fc2;
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for code in extract_censored_codes(&base)
        .into_iter()
        .chain(extract_uncensored_codes(&base))
    {
        append_unique(&mut out, &mut seen, code);
    }
    out
}

/// True when the file name carries a *strong* JAV signal, for the adult
/// classifier.
///
/// Deliberately stricter than [`extract_codes_from_name`]: extraction is
/// recall-oriented because a wrong candidate only costs a failed source
/// lookup, but classification stamps a record as 18+ and moves it into the
/// adult zone, so it must not fire on ordinary titles. The loose patterns
/// match things like "Level 03", "Part 2" or "Movie 2020", which are not
/// reliable 18+ indicators.
pub fn looks_like_jav_name(name: &str) -> bool {
    let base = file_stem(name);
    if base.is_empty() {
        return false;
    }
    // FC2 marketplace ids are always adult.
    if fc2_re().is_match(&base) {
        return true;
    }
    // Pure-numeric uncensored codes such as "051626-001" are always adult.
    if pure_numeric_uncensored_re().is_match(&base) {
        return true;
    }
    // Letter-prefix codes: require a strong number part. The mandatory-
    // separator pattern covers "IPX-633" / "Level 03"-style spaced names; the
    // generic pattern also covers concatenated codes such as "ipx633".
    mixed_prefix_censored_re().captures_iter(&base).any(|caps| {
        caps.get(3)
            .map(|m| m.as_str())
            .is_some_and(is_strong_code_number)
    }) || code_re().captures_iter(&base).any(|caps| {
        caps.get(2)
            .map(|m| m.as_str())
            .is_some_and(is_strong_code_number)
    })
}

/// Ordinary file-name words that can precede a number but are never JAV studio
/// codes. The relaxed classifier uses this to keep titles such as
/// `Level 03` / `Part 14` / `Movie 2020` out of the 18+ zone. Deliberately
/// short and conservative: a missed entry only costs one hidden-zone false
/// positive, whereas an over-eager entry would let real 18+ content leak into
/// the main library.
///
/// `hdr` / `sdr` / `uhd` cover dynamic-range and resolution markers that ride
/// along in release names ("...2160p.HDR10+.mkv"): without them the recall-
/// oriented extractor turns `HDR10` into the code-shaped candidate `HDR-010`
/// and ordinary movies get isolated into the 18+ zone. No JAV studio uses
/// these tokens as a prefix, so they are safe to reject outright.
const NON_JAV_PREFIX_WORDS: &[&str] = &[
    "level", "part", "episode", "movie", "film", "season", "chapter", "scene", "volume", "trailer",
    "sample", "lesson", "course", "lecture", "tutorial", "page", "disc", "disk", "dvd", "hdr",
    "sdr", "uhd",
];

/// True when an extracted candidate code is strong enough to isolate a record
/// as 18+. This is the recall-oriented counterpart of
/// [`is_strong_code_number`]: it accepts two-digit codes (which the strict
/// check rejects as ambiguous) but filters the dominant false positives —
/// release years, common file-name words and video-codec markers.
fn is_isolatable_code(code: &str) -> bool {
    let code = code.trim();
    if code.is_empty() {
        return false;
    }
    let digit_len = code
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .count();
    if digit_len < 2 {
        return false;
    }
    let digits = &code[code.len() - digit_len..];
    // A 4-digit 19xx/20xx number is a release year, not a code.
    if digit_len == 4 && (digits.starts_with("19") || digits.starts_with("20")) {
        return false;
    }
    let prefix =
        code[..code.len() - digit_len].trim_end_matches(|c| c == '-' || c == '_' || c == ' ');
    // The studio-code candidate is the last alphanumeric token of the prefix
    // ("FC2-PPV-1234567" -> "PPV", "051626-001" -> "051626").
    let token = prefix
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or("");
    let token_lower = token.to_ascii_lowercase();
    if token_lower.is_empty() {
        // Pure-numeric uncensored code ("051626-001"): always adult.
        return true;
    }
    // Codec markers (H.264 / H.265 / x264 / x265) are not codes.
    if matches!(token_lower.as_str(), "h" | "x") && matches!(digits, "264" | "265" | "266") {
        return false;
    }
    // The extraction regexes cap the letter prefix at six characters, so a long
    // file-name word arrives truncated ("Episode 05" -> "PISODE-005"). Treat the
    // token as an ordinary word when it equals — or is a trailing fragment of —
    // a known non-JAV word. Fragments shorter than three characters are left
    // alone so real short studio codes ("PT", "DV") keep isolating.
    !NON_JAV_PREFIX_WORDS.iter().any(|word| {
        *word == token_lower || (token_lower.len() >= 3 && word.ends_with(token_lower.as_str()))
    })
}

/// Relaxed 18+ classification signal: a superset of [`looks_like_jav_name`]
/// that also isolates names whose JAV-shaped code the strict check rejects
/// (notably two-digit codes such as `PT-82`), after filtering the dominant
/// false positives.
///
/// Used at import time so adult content is isolated *before* the network-bound
/// scrape confirms it. Without this, an item imported with an ambiguous name
/// sits in the main library until scraping finishes and reclassifies it — the
/// window where 18+ videos leak onto the home page and the adult zone looks
/// empty.
pub fn looks_like_jav_name_relaxed(name: &str) -> bool {
    let base = file_stem(name);
    if base.is_empty() {
        return false;
    }
    if looks_like_jav_name(name) {
        return true;
    }
    extract_codes_from_name(name)
        .iter()
        .any(|code| is_isolatable_code(code))
}

/// A code number part is a strong 18+ signal when it has at least three
/// digits (real codes such as `IPX-633`, `ABP-356`, `MIAA-068`) and is not a
/// plausible year, which keeps ordinary titles like "Movie 2020" out of the
/// adult zone. Two-digit shapes ("Level 03", "Part 2") are too ambiguous.
fn is_strong_code_number(digits: &str) -> bool {
    if digits.len() < 3 {
        return false;
    }
    !(digits.len() == 4 && (digits.starts_with("19") || digits.starts_with("20")))
}

fn file_stem(name: &str) -> String {
    // Cloud providers often expose media through signed download URLs. Query
    // parameters are transport metadata, not part of the media file name; they
    // can contain code-shaped tokens such as `DD-020` and must not affect
    // 18+ classification.
    let path = name.split(['?', '#']).next().unwrap_or(name);
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    Path::new(base)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| base.to_owned())
}

fn extract_censored_codes(base: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for caps in code_re().captures_iter(base) {
        let suffix = caps
            .get(3)
            .map(|m| m.as_str().trim().to_uppercase())
            .unwrap_or_default();
        let number = normalize_number(caps.get(2).map(|m| m.as_str()).unwrap_or(""));
        let code = format!(
            "{}-{}",
            caps.get(1)
                .map(|m| m.as_str().to_uppercase())
                .unwrap_or_default(),
            number
        );
        append_unique(&mut out, &mut seen, code.clone());
        if !suffix.is_empty() {
            append_unique(&mut out, &mut seen, format!("{code}{suffix}"));
        }
    }
    for caps in mixed_prefix_censored_re().captures_iter(base) {
        let suffix = caps
            .get(4)
            .map(|m| m.as_str().trim().to_uppercase())
            .unwrap_or_default();
        let number = normalize_number(caps.get(3).map(|m| m.as_str()).unwrap_or(""));
        let code = format!(
            "{}-{}",
            caps.get(2)
                .map(|m| m.as_str().to_uppercase())
                .unwrap_or_default(),
            number
        );
        append_unique(&mut out, &mut seen, code.clone());
        if !suffix.is_empty() {
            append_unique(&mut out, &mut seen, format!("{code}{suffix}"));
        }
    }
    out
}

fn extract_uncensored_codes(base: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for caps in mixed_prefix_uncensored_re().captures_iter(base) {
        let prefix = caps.get(2).map(|m| m.as_str().trim()).unwrap_or_default();
        let number = caps.get(3).map(|m| m.as_str().trim()).unwrap_or_default();
        if prefix.is_empty() || number.is_empty() {
            continue;
        }
        append_unique(&mut out, &mut seen, format!("{prefix}-{number}"));
    }

    for caps in alpha_numeric_uncensored_re().captures_iter(base) {
        let prefix =
            normalize_uncensored_alpha_prefix(caps.get(2).map(|m| m.as_str()).unwrap_or(""));
        let separator = caps.get(3).map(|m| m.as_str().trim()).unwrap_or_default();
        let number = caps.get(4).map(|m| m.as_str().trim()).unwrap_or_default();
        if prefix.is_empty() || number.is_empty() {
            continue;
        }
        if separator.is_empty() {
            append_unique(&mut out, &mut seen, format!("{prefix}{number}"));
        }
        if !separator.is_empty() || prefix.chars().count() > 1 {
            append_unique(&mut out, &mut seen, format!("{prefix}-{number}"));
        }
    }

    for caps in pure_numeric_uncensored_re().captures_iter(base) {
        let number = caps.get(2).map(|m| m.as_str().trim()).unwrap_or_default();
        if !number.is_empty() {
            append_unique(&mut out, &mut seen, number.to_owned());
        }
    }

    for caps in explicit_short_code_re().captures_iter(base) {
        let prefix = caps
            .get(2)
            .map(|m| m.as_str().trim().to_uppercase())
            .unwrap_or_default();
        let number = caps
            .get(3)
            .map(|m| m.as_str().trim().to_uppercase())
            .unwrap_or_default();
        let suffix = caps
            .get(4)
            .map(|m| m.as_str().trim().to_uppercase())
            .unwrap_or_default();
        if prefix.is_empty() || number.is_empty() {
            continue;
        }
        let code = format!("{prefix}-{number}");
        append_unique(&mut out, &mut seen, code.clone());
        if !suffix.is_empty() {
            append_unique(&mut out, &mut seen, format!("{code}{suffix}"));
        }
    }
    out
}

fn append_unique(out: &mut Vec<String>, seen: &mut HashSet<String>, code: String) {
    let code = code.trim().to_uppercase();
    if code.is_empty() || !seen.insert(code.clone()) {
        return;
    }
    out.push(code);
}

/// Single-letter prefixes become lower-case (`h`), longer prefixes become
/// title-case (`Fc2`); the final upper-casing in `append_unique` normalizes
/// them for lookup anyway.
fn normalize_uncensored_alpha_prefix(prefix: &str) -> String {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return String::new();
    }
    let mut chars = prefix.chars();
    let first = chars.next().expect("non-empty").to_uppercase().to_string();
    if chars.as_str().is_empty() {
        return prefix.to_lowercase();
    }
    format!("{}{}", first, chars.as_str().to_lowercase())
}

/// Trim leading zeros but keep at least three digits (pad back if needed),
/// mirroring JavBoss `normalizeNumber`.
fn normalize_number(num: &str) -> String {
    let trimmed = num.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    if trimmed.len() < 3 {
        format!("{:0>3}", trimmed).to_uppercase()
    } else {
        trimmed.to_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_codes_from_name, looks_like_jav_name, looks_like_jav_name_relaxed};

    fn check(input: &str, expected: &[&str]) {
        let actual = extract_codes_from_name(input);
        let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(actual, expected, "input: {input}");
    }

    // Cases ported from JavBoss internal/util/jav_util_test.go.
    #[test]
    fn extracts_common_censored_codes() {
        check("1024核工厂-ABP-356.HD.mp4", &["ABP-356"]);
        check("ABP-178.avi", &["ABP-178"]);
        check("ABP-888C.mp4", &["ABP-888", "ABP-888C"]);
        check("abp-782_1.mp4", &["ABP-782"]);
        check("AGEMIX-080.avi", &["AGEMIX-080"]);
        check("AMBI027a.avi", &["AMBI-027", "AMBI-027A"]);
        check("[44x.me]AOZ-132-C.mp4", &["AOZ-132"]);
        check("IBW-938z.mp4", &["IBW-938", "IBW-938Z"]);
        check("BANK-002 S .mp4", &["BANK-002"]);
        check("dv-1530_0.avi", &["DV-1530"]);
        check("dv-1448.mp4", &["DV-1448"]);
        check("IBW572Z.mp4", &["IBW-572", "IBW-572Z"]);
        check("PT-82.mp4", &["PT-082", "PT-82"]);
        check("LLDV-44.mp4", &["LLDV-044", "LLDV-44"]);
    }

    #[test]
    fn extracts_codes_without_separators() {
        check("SDNM256.mp4", &["SDNM-256", "SDNM256"]);
        check("PKPD066C.mp4", &["PKPD-066", "PKPD-066C"]);
        check("miaa00068.mp4", &["MIAA-068", "MIAA00068", "MIAA-00068"]);
        check("ovg00129.mp4", &["OVG-129", "OVG00129", "OVG-00129"]);
        check("ipx00633.mp4", &["IPX-633", "IPX00633", "IPX-00633"]);
    }

    #[test]
    fn extracts_spaced_and_uncensored_codes() {
        check("Heyzo-0945-HD.mp4", &["HEYZO-945", "HEYZO-0945"]);
        check(
            "Heyzo - 0945 - Reiko Kobayakawa (小早川怜子).mp4",
            &["HEYZO-0945"],
        );
        check(
            "HEYZO 0945 美痴女～爆乳弁護士に責められる～ - 小早川怜子 [UNCENSORED].mp4",
            &["HEYZO-945", "HEYZO0945", "HEYZO-0945"],
        );
        check("Tokyo Hot n0646.avi", &["N0646"]);
        check("051626-001-CARIB.mp4", &["051626-001"]);
        check(
            "031419_001-1pon-1080p.mp4",
            &["PON-1080", "PON-1080P", "031419_001"],
        );
        check(
            "heydouga-4030-2296.mp4",
            &["YDOUGA-4030", "HEYDOUGA-4030", "4030-2296"],
        );
        check("t28-502.mp4", &["T28-502", "T28"]);
        check(
            "javcn_MCB3DBD-42-H265.mp4",
            &["DBD-042", "MCB3DBD-42", "H265"],
        );
        check("FC2-PPV-1234567.mp4", &["FC2-PPV-1234567", "FC2-1234567"]);
        check("FC2-1234567.mp4", &["FC2-1234567"]);
        check(
            "fc2_ppv_2588575_1080p.mp4",
            &["FC2-PPV-2588575", "FC2-2588575"],
        );
    }

    #[test]
    fn ignores_plain_titles() {
        assert!(extract_codes_from_name("Breaking.Bad.S01E01.1080p.mkv").is_empty());
        assert!(extract_codes_from_name("新闻联播.mp4").is_empty());
        assert!(extract_codes_from_name("").is_empty());
    }

    #[test]
    fn classification_flags_strong_jav_signals() {
        assert!(looks_like_jav_name("IPX-633.mp4"));
        assert!(looks_like_jav_name("ipx633.mp4"));
        assert!(looks_like_jav_name("ABP-356.HD.mp4"));
        assert!(looks_like_jav_name("MIAA-068.mp4"));
        assert!(looks_like_jav_name("dv-1530_0.avi"));
        assert!(looks_like_jav_name("Heyzo-0945-HD.mp4"));
        assert!(looks_like_jav_name("FC2-PPV-1234567.mp4"));
        assert!(looks_like_jav_name("051626-001-CARIB.mp4"));
    }

    #[test]
    fn classification_ignores_common_false_positives() {
        // Two-digit "word + number" shapes are not reliable 18+ signals.
        assert!(!looks_like_jav_name("小狐狸分级 - Level 03.mp4"));
        assert!(!looks_like_jav_name("Part 2.mp4"));
        // Years in ordinary titles must not be treated as codes.
        assert!(!looks_like_jav_name("Movie 2020.mp4"));
        assert!(!looks_like_jav_name("Film.1999.1080p.mkv"));
        // Codec names and episode markers are not codes.
        assert!(!looks_like_jav_name("Some.Movie.H265.mp4"));
        assert!(!looks_like_jav_name("Breaking.Bad.S01E01.1080p.mkv"));
        assert!(!looks_like_jav_name("新闻联播.mp4"));
        assert!(!looks_like_jav_name(""));
    }

    #[test]
    fn relaxed_classification_is_a_superset_of_strict() {
        // Everything the strict check flags, the relaxed check also flags.
        for name in [
            "IPX-633.mp4",
            "ipx633.mp4",
            "ABP-356.HD.mp4",
            "MIAA-068.mp4",
            "dv-1530_0.avi",
            "Heyzo-0945-HD.mp4",
            "FC2-PPV-1234567.mp4",
            "051626-001-CARIB.mp4",
        ] {
            assert!(looks_like_jav_name_relaxed(name), "input: {name}");
        }
    }

    #[test]
    fn relaxed_classification_catches_two_digit_codes() {
        // Two-digit codes the strict check rejects as ambiguous are still
        // isolated by the relaxed check — this closes the import-time leak
        // window for titles like "PT-82" / "LLDV-44".
        assert!(looks_like_jav_name_relaxed("PT-82.mp4"));
        assert!(looks_like_jav_name_relaxed("LLDV-44.mp4"));
        assert!(looks_like_jav_name_relaxed("ibw-93.mp4"));
    }

    #[test]
    fn relaxed_classification_avoids_common_false_positives() {
        // Years, common file-name words and codec markers stay out of the 18+
        // zone even under the relaxed check.
        assert!(!looks_like_jav_name_relaxed("Movie 2020.mp4"));
        assert!(!looks_like_jav_name_relaxed("Film.1999.1080p.mkv"));
        assert!(!looks_like_jav_name_relaxed("小狐狸分级 - Level 03.mp4"));
        assert!(!looks_like_jav_name_relaxed("Part 14.mp4"));
        assert!(!looks_like_jav_name_relaxed("Episode 05.mp4"));
        assert!(!looks_like_jav_name_relaxed("Some.Movie.H265.mp4"));
        assert!(!looks_like_jav_name_relaxed("Family.Movie.1080p.mp4"));
        assert!(!looks_like_jav_name_relaxed(
            "Breaking.Bad.S01E01.1080p.mkv"
        ));
        assert!(!looks_like_jav_name_relaxed("新闻联播.mp4"));
        assert!(!looks_like_jav_name_relaxed(""));
    }

    #[test]
    fn ignores_code_shaped_tokens_in_signed_url_queries() {
        // Cloud-drive download signatures may contain arbitrary values that
        // happen to look like a JAV code. Only the media path is relevant.
        let url = "https://media.example/download?token=DD-020&expires=1780000000";
        assert!(!looks_like_jav_name_relaxed(url));
        assert!(extract_codes_from_name(url).is_empty());
    }

    #[test]
    fn relaxed_classification_ignores_dynamic_range_and_resolution_markers() {
        // "HDR10"/"HDR10+"/"SDR"/"UHD" in a release name extract the code-shaped
        // candidate "HDR-010" (etc.), but that must never isolate an ordinary
        // movie into the 18+ zone — regression for the "007：大破量子危机"
        // false positive that waited on JavBus forever.
        let names = [
            "007：大破量子危机 (2008) - 2160p.HDR10+.mkv",
            "007：大破量子危机 (2008) - 2160p.HDR10.mkv",
            "大破天幕杀机 (2012) - 1080p.HDR.mkv",
            "Some.Movie.2160p.UHD.BluRay.mkv",
            "Movie.1080p.SDR.mkv",
        ];
        for name in names {
            assert!(looks_like_jav_name_relaxed(name) == false, "input: {name}");
        }
        // Extraction itself stays recall-oriented: candidates are still
        // produced (they only cost a failed source lookup once a *real* code
        // routes the item into the JAV branch anyway).
        assert_eq!(
            extract_codes_from_name("Movie.2160p.HDR10+.mkv"),
            ["HDR-010", "HDR10", "HDR-10"]
        );
    }
}
