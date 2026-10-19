//! Text normalisation: numbers, money, percentages, times, ordinals, abbreviations and symbols become the words a
//! British speaker would say, before phonemes are looked up. The output is a list of word and punctuation tokens.

/// One piece of normalised text.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Word(String),
    /// one of `. , ! ? ; : — …`
    Punct(char),
}

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const SCALES: [&str; 5] = ["", "thousand", "million", "billion", "trillion"];

fn below_hundred(n: u64) -> String {
    if n < 20 {
        ONES[n as usize].to_string()
    } else if n.is_multiple_of(10) {
        TENS[(n / 10) as usize].to_string()
    } else {
        format!("{}-{}", TENS[(n / 10) as usize], ONES[(n % 10) as usize])
    }
}

fn below_thousand(n: u64) -> String {
    match (n / 100, n % 100) {
        (0, r) => below_hundred(r),
        (h, 0) => format!("{} hundred", ONES[h as usize]),
        (h, r) => format!("{} hundred and {}", ONES[h as usize], below_hundred(r)),
    }
}

/// British cardinal: 1,234 is "one thousand, two hundred and thirty-four" (without the comma).
pub fn cardinal(n: u64) -> String {
    if n == 0 {
        return "zero".into();
    }
    if n >= 1_000_000_000_000_000 {
        return digits(&n.to_string());
    }
    let mut groups = Vec::new();
    let mut m = n;
    while m > 0 {
        groups.push(m % 1000);
        m /= 1000;
    }
    let mut parts = Vec::new();
    for (i, g) in groups.iter().enumerate().rev() {
        if *g == 0 {
            continue;
        }
        let mut s = below_thousand(*g);
        // "one thousand and five": the last group gets "and" when it has no hundreds
        if i == 0 && groups.len() > 1 && *g < 100 {
            s = format!("and {s}");
        }
        if i > 0 {
            s = format!("{s} {}", SCALES[i]);
        }
        parts.push(s);
    }
    parts.join(" ")
}

/// "first", "twenty-second", "one hundredth".
pub fn ordinal(n: u64) -> String {
    let c = cardinal(n);
    let (head, last) = match c.rfind([' ', '-']) {
        Some(i) => (&c[..=i], &c[i + 1..]),
        None => ("", c.as_str()),
    };
    let last = match last {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    };
    format!("{head}{last}")
}

/// Years the way they are said: 1984 "nineteen eighty-four", 2005 "two thousand and five", 2026 "twenty twenty-six".
pub fn year(n: u64) -> String {
    let (hi, lo) = (n / 100, n % 100);
    if (2000..2010).contains(&n) || n.is_multiple_of(1000) {
        cardinal(n)
    } else if lo == 0 {
        format!("{} hundred", below_hundred(hi))
    } else if lo < 10 {
        format!("{} oh {}", below_hundred(hi), ONES[lo as usize])
    } else {
        format!("{} {}", below_hundred(hi), below_hundred(lo))
    }
}

/// Digit by digit, with "oh" for zero.
pub fn digits(s: &str) -> String {
    s.chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| if d == 0 { "oh" } else { ONES[d as usize] })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A decimal like 3.14: "three point one four" (digits after the point are read one by one, zeros as "zero").
fn decimal(int: &str, frac: &str) -> String {
    let i = if int.is_empty() {
        "nought".to_string()
    } else {
        cardinal(int.parse().unwrap_or(0))
    };
    let f: Vec<&str> = frac
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| ONES[d as usize])
        .collect();
    if f.is_empty() {
        i
    } else {
        format!("{i} point {}", f.join(" "))
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", cardinal(n), if n == 1 { one } else { many })
}

/// Spoken form of a run of digits (with commas and one point allowed).
fn number_words(raw: &str) -> String {
    let s: String = raw.chars().filter(|c| *c != ',').collect();
    if let Some((i, f)) = s.split_once('.') {
        return decimal(i, f);
    }
    if s.len() > 1 && s.starts_with('0') {
        return digits(&s);
    }
    match s.parse::<u64>() {
        // four-digit numbers without a comma read as years
        Ok(n) if !raw.contains(',') && s.len() == 4 && (1100..2100).contains(&n) => year(n),
        Ok(n) => cardinal(n),
        Err(_) => digits(&s),
    }
}

const SCALE_WORDS: [&str; 5] = ["hundred", "thousand", "million", "billion", "trillion"];

fn currency(sym: char) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    match sym {
        '$' => Some(("dollar", "dollars", "cent", "cents")),
        '£' => Some(("pound", "pounds", "penny", "pence")),
        '€' => Some(("euro", "euros", "cent", "cents")),
        _ => None,
    }
}

/// Money: "$12.50" "twelve dollars and fifty cents"; `scale` (e.g. "million") goes before the unit.
fn money(sym: char, raw: &str, scale: Option<&str>) -> String {
    let (one, many, sub1, subn) = currency(sym).unwrap_or(("", "", "", ""));
    let s: String = raw.chars().filter(|c| *c != ',').collect();
    if let Some(sc) = scale {
        let n = if s.contains('.') {
            number_words(&s)
        } else {
            cardinal(s.parse().unwrap_or(0))
        };
        return format!("{n} {sc} {many}");
    }
    let (i, f) = s.split_once('.').unwrap_or((&s, ""));
    let whole: u64 = i.parse().unwrap_or(0);
    let cents: u64 = if f.is_empty() {
        0
    } else {
        format!("{:0<2}", &f[..f.len().min(2)]).parse().unwrap_or(0)
    };
    match (whole, cents) {
        (w, 0) => plural(w, one, many),
        (0, c) => plural(c, sub1, subn),
        (w, c) => format!("{} and {}", plural(w, one, many), plural(c, sub1, subn)),
    }
}

/// Abbreviations that are read as words (with or without their full stop).
fn abbreviation(w: &str) -> Option<&'static str> {
    Some(match w {
        "Mr" => "mister",
        "Mrs" => "missus",
        "Ms" => "miz",
        "Dr" => "doctor",
        "St" => "saint",
        "Prof" => "professor",
        "vs" => "versus",
        "etc" => "et cetera",
        "e.g" | "eg" => "for example",
        "i.e" | "ie" => "that is",
        "approx" => "approximately",
        "km" => "kilometres",
        "kg" => "kilograms",
        "ms" => "milliseconds",
        "MB" => "megabytes",
        "GB" => "gigabytes",
        "KB" => "kilobytes",
        "TB" => "terabytes",
        _ => return None,
    })
}

fn symbol(c: char) -> Option<&'static str> {
    Some(match c {
        '&' => "and",
        '+' => "plus",
        '@' => "at",
        '=' => "equals",
        '%' => "percent",
        '#' => "number",
        '~' => "about",
        _ => return None,
    })
}

fn punct(c: char) -> Option<char> {
    match c {
        '.' | ',' | '!' | '?' | ';' | ':' | '…' => Some(c),
        '—' | '–' => Some('—'),
        _ => None,
    }
}

fn push_words(out: &mut Vec<Tok>, s: &str) {
    for w in s.split([' ', '-']).filter(|w| !w.is_empty()) {
        out.push(Tok::Word(w.to_string()));
    }
}

/// Splits and normalises text into word and punctuation tokens.
pub fn normalise(text: &str) -> Vec<Tok> {
    let text = text.replace(['\u{2018}', '\u{2019}', '`'], "'");
    let cs: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace()
            || c == '"'
            || c == '\u{201c}'
            || c == '\u{201d}'
            || c == '('
            || c == ')'
            || c == '['
            || c == ']'
        {
            // parentheses read as a short pause
            if (c == '(' || c == ')') && !matches!(out.last(), Some(Tok::Punct(_)) | None) {
                out.push(Tok::Punct(','));
            }
            i += 1;
            continue;
        }
        // money
        if currency(c).is_some() && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            let start = i + 1;
            let mut j = start;
            while j < cs.len()
                && (cs[j].is_ascii_digit()
                    || ((cs[j] == ',' || cs[j] == '.')
                        && cs.get(j + 1).is_some_and(|d| d.is_ascii_digit())))
            {
                j += 1;
            }
            let raw: String = cs[start..j].iter().collect();
            // "$1.5 million", "$3bn", "$20k"
            let mut scale = None;
            let rest: String = cs[j..].iter().take(12).collect();
            let rest_l = rest.to_lowercase();
            let mut skip = 0;
            for (abbr, word) in [
                ("k", "thousand"),
                ("m", "million"),
                ("bn", "billion"),
                ("b", "billion"),
                ("tn", "trillion"),
            ] {
                if rest_l.starts_with(abbr)
                    && !rest_l[abbr.len()..].starts_with(|c: char| c.is_alphanumeric())
                {
                    scale = Some(word);
                    skip = abbr.len();
                    break;
                }
            }
            if scale.is_none() {
                let next: String = rest_l
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphabetic())
                    .collect();
                if let Some(w) = SCALE_WORDS.iter().find(|w| **w == next) {
                    scale = Some(w);
                    skip = rest_l.len() - rest_l.trim_start().len() + next.len();
                }
            }
            push_words(&mut out, &money(c, &raw, scale));
            i = j + skip;
            continue;
        }
        // numbers, with their suffixes (%, st/nd/rd/th, s) and times (10:30)
        if c.is_ascii_digit()
            || (c == '.'
                && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())
                && !cs
                    .get(i.wrapping_sub(1))
                    .is_some_and(|d| d.is_alphanumeric()))
        {
            let start = i;
            let mut j = i;
            while j < cs.len()
                && (cs[j].is_ascii_digit()
                    || ((cs[j] == ',' || cs[j] == '.' || cs[j] == ':')
                        && cs.get(j + 1).is_some_and(|d| d.is_ascii_digit())))
            {
                j += 1;
            }
            let raw: String = cs[start..j].iter().collect();
            let suffix: String = cs[j..].iter().take_while(|c| c.is_alphabetic()).collect();
            let sl = suffix.to_lowercase();
            if let Some((h, m)) = raw
                .split_once(':')
                .filter(|(h, m)| !m.contains(':') && h.len() <= 2 && m.len() == 2)
            {
                let (h, m): (u64, u64) = (h.parse().unwrap_or(0), m.parse().unwrap_or(0));
                let mins = match m {
                    0 => String::new(),
                    1..=9 => format!(" oh {}", ONES[m as usize]),
                    _ => format!(" {}", below_hundred(m)),
                };
                let when = match sl.as_str() {
                    "am" => " a m",
                    "pm" => " p m",
                    _ => "",
                };
                push_words(
                    &mut out,
                    &format!(
                        "{}{}{}",
                        if m == 0 && when.is_empty() {
                            format!("{} o'clock", cardinal(h))
                        } else {
                            cardinal(h)
                        },
                        mins,
                        when
                    ),
                );
                i = j + if when.is_empty() { 0 } else { suffix.len() };
                continue;
            }
            if raw.matches('.').count() > 1 {
                // a version: 1.2.3 is "one point two point three"
                let parts: Vec<String> = raw.split('.').map(number_words).collect();
                push_words(&mut out, &parts.join(" point "));
                i = j;
                continue;
            }
            let plain: String = raw.chars().filter(|c| *c != ',').collect();
            if matches!(sl.as_str(), "st" | "nd" | "rd" | "th") && !plain.contains('.') {
                push_words(&mut out, &ordinal(plain.parse().unwrap_or(0)));
                i = j + suffix.len();
                continue;
            }
            let mut words = number_words(&raw);
            let mut used = 0;
            if cs.get(j) == Some(&'%') {
                words.push_str(" percent");
                used = 1;
            } else if sl == "s" && plain.len() == 4 {
                // the 1980s
                words = match words.strip_suffix('y') {
                    Some(w) => format!("{w}ies"),
                    None => format!("{words}s"),
                };
                used = 1;
            } else if let Some(unit) = abbreviation(&suffix).filter(|_| !suffix.is_empty()) {
                words = format!("{words} {unit}");
                used = suffix.len();
            } else if sl == "x" {
                words.push_str(" times");
                used = 1;
            }
            push_words(&mut out, &words);
            i = j + used;
            continue;
        }
        if c.is_alphabetic() {
            let start = i;
            let mut j = i;
            while j < cs.len()
                && (cs[j].is_alphanumeric()
                    || (cs[j] == '\'' && cs.get(j + 1).is_some_and(|d| d.is_alphabetic())))
            {
                if cs[j].is_ascii_digit() {
                    break;
                }
                j += 1;
            }
            let w: String = cs[start..j].iter().collect();
            // "e.g." and "i.e."
            if (w == "e" || w == "i") && cs.get(j) == Some(&'.') && cs.get(j + 2) == Some(&'.') {
                let pair: String = cs[start..j + 2].iter().collect();
                if let Some(a) = abbreviation(&pair) {
                    push_words(&mut out, a);
                    i = j + 3;
                    out.push(Tok::Punct(','));
                    continue;
                }
            }
            if let Some(a) = abbreviation(&w) {
                push_words(&mut out, a);
                // a title's own full stop is not the end of a sentence ("etc." keeps its, it often ends one)
                let titled = matches!(
                    w.as_str(),
                    "Mr" | "Mrs" | "Ms" | "Dr" | "St" | "Prof" | "vs" | "approx"
                );
                i = if titled && cs.get(j) == Some(&'.') {
                    j + 1
                } else {
                    j
                };
                continue;
            }
            out.push(Tok::Word(w));
            i = j;
            continue;
        }
        if c == '\'' {
            i += 1;
            continue;
        }
        if let Some(p) = punct(c) {
            // "..." is an ellipsis
            if c == '.' && cs.get(i + 1) == Some(&'.') {
                while i < cs.len() && cs[i] == '.' {
                    i += 1;
                }
                out.push(Tok::Punct('…'));
                continue;
            }
            // a dot inside a word (file.rs, sphere.dev) is "dot"
            if c == '.'
                && i > 0
                && cs[i - 1].is_alphanumeric()
                && cs.get(i + 1).is_some_and(|d| d.is_alphanumeric())
            {
                out.push(Tok::Word("dot".into()));
                i += 1;
                continue;
            }
            // a hyphen-like dash between words with no spaces is just a join
            if !matches!(out.last(), Some(Tok::Punct(_)) | None) {
                out.push(Tok::Punct(p));
            }
            i += 1;
            continue;
        }
        if c == '/' {
            // "and/or" reads "and or"; a lone slash is silent
            i += 1;
            continue;
        }
        if c == '-' {
            // a spaced hyphen is a dash; inside a word it just joins
            let spaced = cs.get(i.wrapping_sub(1)).is_none_or(|d| d.is_whitespace())
                && cs.get(i + 1).is_none_or(|d| d.is_whitespace());
            if spaced && !matches!(out.last(), Some(Tok::Punct(_)) | None) {
                out.push(Tok::Punct('—'));
            } else if !spaced
                && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())
                && cs.get(i.wrapping_sub(1)).is_none_or(|d| d.is_whitespace())
            {
                out.push(Tok::Word("minus".into()));
            }
            i += 1;
            continue;
        }
        if let Some(s) = symbol(c) {
            push_words(&mut out, s);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `s` normalises to, with hyphenated number words written joined for readability.
    fn said(s: &str) -> String {
        let words: Vec<String> = normalise(s)
            .into_iter()
            .map(|t| match t {
                Tok::Word(w) => w,
                Tok::Punct(p) => p.to_string(),
            })
            .collect();
        let mut out = words.join(" ");
        for (t, o) in TENS
            .iter()
            .skip(2)
            .flat_map(|t| ONES[1..10].iter().map(move |o| (t, o)))
        {
            out = out.replace(&format!("{t} {o}"), &format!("{t}-{o}"));
        }
        out
    }

    #[test]
    fn cardinals_are_british() {
        assert_eq!(cardinal(0), "zero");
        assert_eq!(cardinal(45), "forty-five");
        assert_eq!(cardinal(345), "three hundred and forty-five");
        assert_eq!(cardinal(1005), "one thousand and five");
        assert_eq!(cardinal(3400), "three thousand four hundred");
        assert_eq!(cardinal(2_500_000), "two million five hundred thousand");
        assert_eq!(ordinal(1), "first");
        assert_eq!(ordinal(22), "twenty-second");
        assert_eq!(ordinal(40), "fortieth");
        assert_eq!(year(1984), "nineteen eighty-four");
        assert_eq!(year(2005), "two thousand and five");
        assert_eq!(year(2026), "twenty twenty-six");
        assert_eq!(year(1900), "nineteen hundred");
    }

    #[test]
    fn money_numbers_and_symbols() {
        assert_eq!(
            said("It costs $12.50."),
            "It costs twelve dollars and fifty cents ."
        );
        assert_eq!(said("$1 and £3"), "one dollar and three pounds");
        assert_eq!(said("$0.99"), "ninety-nine cents");
        assert_eq!(said("$1.5 million"), "one point five million dollars");
        assert_eq!(said("$20k"), "twenty thousand dollars");
        assert_eq!(said("45% done"), "forty-five percent done");
        assert_eq!(said("3.5%"), "three point five percent");
        assert_eq!(said("the 3rd run"), "the third run");
        assert_eq!(said("at 10:30"), "at ten thirty");
        assert_eq!(said("at 9:05pm"), "at nine oh five p m");
        assert_eq!(said("v1.2.3"), "v one point two point three");
        assert_eq!(said("in 2026"), "in twenty twenty-six");
        assert_eq!(
            said("1,234 files"),
            "one thousand two hundred and thirty-four files"
        );
        assert_eq!(said("12 tests & 3 agents"), "twelve tests and three agents");
        assert_eq!(said("8GB"), "eight gigabytes");
    }

    #[test]
    fn words_punctuation_and_abbreviations() {
        assert_eq!(
            said("Good evening, sir. Shall I?"),
            "Good evening , sir . Shall I ?"
        );
        assert_eq!(
            said("Dr. Smith vs. the world"),
            "doctor Smith versus the world"
        );
        assert_eq!(said("don't stop"), "don't stop");
        assert_eq!(said("main.rs - ok"), "main dot rs — ok");
        assert_eq!(said("wait..."), "wait …");
        assert_eq!(said("e.g. this"), "for example , this");
    }
}
