//! Grapheme to phoneme, British English, in Kokoro's phoneme alphabet. A port of the parts of misaki
//! (github.com/hexgrad/misaki, Apache-2.0) that need no neural networks: its gold and silver British lexicons,
//! stress rules, the -s/-ed/-ing stemming and the function-word special cases. Words the lexicons don't know go to
//! the `espeak-ng` program when it is installed (run as a separate process, never linked), then to spelling rules.
use crate::text::{self, Tok};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

const PRIMARY: char = 'ˈ';
const SECONDARY: char = 'ˌ';
const VOWELS: &str = "AIOQWYaiuæɑɒɔəɛɜɪʊʌᵻ";
const CONSONANTS: &str = "bdfhjklmnpstvwzðŋɡɹɾʃʒʤʧθ";
/// misaki's British phoneme set (plus ɐ for "a")
const GB_VOCAB: &str = "AIQWYabdfhijklmnpstuvwzðŋɑɒɔəɛɜɡɪɹʃʊʌʒʤʧˈˌːθᵊɐ";

fn is_vowel(c: char) -> bool {
    VOWELS.contains(c)
}

/// The British lexicons in one sorted blob: lines of `word \t phonemes [\t phonemes-at-end-of-phrase]`.
pub struct Lexicon {
    blob: String,
    lines: Vec<u32>,
}

impl Lexicon {
    /// Builds the compact lexicon from misaki's gb_gold.json and gb_silver.json (gold wins).
    pub fn compile(gold_json: &str, silver_json: &str) -> Result<String, String> {
        let mut all: std::collections::BTreeMap<String, String> = Default::default();
        for src in [silver_json, gold_json] {
            let v: serde_json::Value = serde_json::from_str(src).map_err(|e| e.to_string())?;
            let obj = v.as_object().ok_or("lexicon is not an object")?;
            for (k, v) in obj {
                if k.contains(['\t', '\n']) {
                    continue;
                }
                let line = match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(m) => {
                        let Some(d) = m.get("DEFAULT").and_then(|d| d.as_str()) else {
                            continue;
                        };
                        match m.get("None").and_then(|d| d.as_str()) {
                            Some(n) => format!("{d}\t{n}"),
                            None => d.to_string(),
                        }
                    }
                    _ => continue,
                };
                all.insert(k.clone(), line);
            }
        }
        let mut out = String::with_capacity(all.len() * 24);
        for (k, v) in all {
            out.push_str(&k);
            out.push('\t');
            out.push_str(&v);
            out.push('\n');
        }
        Ok(out)
    }

    /// Loads a lexicon made by `compile` (sorted by byte order).
    pub fn from_compiled(blob: String) -> Lexicon {
        let mut lines = Vec::with_capacity(blob.len() / 20);
        let mut at = 0u32;
        for l in blob.split_inclusive('\n') {
            if l.contains('\t') {
                lines.push(at);
            }
            at += l.len() as u32;
        }
        Lexicon { blob, lines }
    }

    fn line(&self, i: usize) -> &str {
        let s = &self.blob[self.lines[i] as usize..];
        &s[..s.find('\n').unwrap_or(s.len())]
    }

    /// The word's phonemes, and its phrase-final form when it has one.
    pub fn get(&self, word: &str) -> Option<(&str, Option<&str>)> {
        let i = self.lines.binary_search_by(|&off| {
            let s = &self.blob[off as usize..];
            s[..s.find('\t').unwrap_or(0)].cmp(word)
        });
        let l = self.line(i.ok()?);
        let mut parts = l.split('\t').skip(1);
        let ps = parts.next()?;
        Some((ps, parts.next()))
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn known(&self, w: &str) -> bool {
        w.len() > 1
            && self
                .get(w)
                .or_else(|| self.get(&w.to_lowercase()))
                .is_some()
    }

    /// Looks a word up as misaki does: exact, then lower case, then capitalised.
    fn find(&self, w: &str) -> Option<(&str, Option<&str>)> {
        if let Some(p) = self.get(w) {
            return Some(p);
        }
        let lower = w.to_lowercase();
        if lower != w {
            if let Some(p) = self.get(&lower) {
                return Some(p);
            }
        }
        let mut cap = lower.clone();
        if let Some(f) = cap.get_mut(0..1) {
            f.make_ascii_uppercase();
        }
        if cap != w {
            return self.get(&cap);
        }
        None
    }
}

/// misaki's `apply_stress`: `stress` < -1 strips stress, -1 demotes, 0..1 adds secondary, >1 adds primary.
fn apply_stress(ps: &str, stress: f32) -> String {
    let has = |c| ps.contains(c);
    if stress < -1.0 {
        ps.replace([PRIMARY, SECONDARY], "")
    } else if stress == -1.0 || ((stress == 0.0 || stress == -0.5) && has(PRIMARY)) {
        ps.replace(SECONDARY, "")
            .replace(PRIMARY, &SECONDARY.to_string())
    } else if (stress == 0.0 || stress == 0.5 || stress == 1.0) && !has(PRIMARY) && !has(SECONDARY)
    {
        restress(ps, SECONDARY)
    } else if stress >= 1.0 && !has(PRIMARY) && has(SECONDARY) {
        ps.replace(SECONDARY, &PRIMARY.to_string())
    } else if stress > 1.0 && !has(PRIMARY) && !has(SECONDARY) {
        restress(ps, PRIMARY)
    } else {
        ps.to_string()
    }
}

/// Puts a stress mark just before the first vowel.
fn restress(ps: &str, mark: char) -> String {
    match ps.char_indices().find(|(_, c)| is_vowel(*c)) {
        Some((i, _)) => format!("{}{mark}{}", &ps[..i], &ps[i..]),
        None => ps.to_string(),
    }
}

fn plural_s(stem: &str) -> String {
    match stem.chars().last() {
        Some(c) if "ptkfθ".contains(c) => format!("{stem}s"),
        Some(c) if "szʃʒʧʤ".contains(c) => format!("{stem}ɪz"),
        _ => format!("{stem}z"),
    }
}

fn past_ed(stem: &str) -> String {
    match stem.chars().last() {
        Some(c) if "pkfθʃsʧ".contains(c) => format!("{stem}t"),
        Some('d') | Some('t') => format!("{stem}ɪd"),
        _ => format!("{stem}d"),
    }
}

/// Letter groups and their sounds, longest first.
#[rustfmt::skip]
const DIGRAPHS: &[(&str, &str)] = &[
    ("tion", "ʃən"), ("sion", "ʒən"), ("ture", "ʧə"), ("ough", "ɔː"), ("igh", "I"), ("tch", "ʧ"), ("dge", "ʤ"), ("sch", "sk"),
    ("ph", "f"), ("sh", "ʃ"), ("ch", "ʧ"), ("th", "θ"), ("wh", "w"), ("ck", "k"), ("ng", "ŋ"), ("qu", "kw"), ("ee", "iː"),
    ("ea", "iː"), ("oo", "uː"), ("ai", "A"), ("ay", "A"), ("ei", "A"), ("ey", "A"), ("oa", "Q"), ("oi", "Y"), ("oy", "Y"),
    ("ou", "W"), ("ow", "Q"), ("au", "ɔː"), ("aw", "ɔː"), ("ew", "juː"), ("ie", "iː"), ("ar", "ɑː"), ("or", "ɔː"), ("er", "ɜː"),
    ("ir", "ɜː"), ("ur", "ɜː"),
];

/// Sounds for letters when nothing else knows a word: a small set of English spelling rules.
fn spelling_rules(word: &str) -> String {
    let w: Vec<char> = word
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect();
    let n = w.len();
    let at = |i: usize| w.get(i).copied().unwrap_or(' ');
    let vowel = |c: char| "aeiouy".contains(c);
    let mut out = String::new();
    let mut i = 0;
    // a final silent e makes the vowel before it long: "bake", "ride", "code"
    let magic =
        |i: usize| n >= 3 && at(n - 1) == 'e' && i + 2 == n - 1 && !vowel(at(i + 1)) && i + 1 < n;
    while i < n {
        let rest: String = w[i..].iter().collect();
        // a final "er" is a schwa: "server"
        let digraph = if rest == "er" {
            Some(("er", "ə"))
        } else {
            DIGRAPHS.iter().find(|(g, _)| rest.starts_with(g)).copied()
        };
        if let Some((g, p)) = digraph {
            out.push_str(p);
            i += g.len();
            continue;
        }
        let c = at(i);
        let p = match c {
            'a' => {
                if magic(i) {
                    "A"
                } else {
                    "a"
                }
            }
            'e' => {
                if i + 1 == n && n > 2 {
                    ""
                } else if magic(i) {
                    "iː"
                } else {
                    "ɛ"
                }
            }
            'i' => {
                if magic(i) {
                    "I"
                } else {
                    "ɪ"
                }
            }
            'o' => {
                if magic(i) || i + 1 == n {
                    "Q"
                } else {
                    "ɒ"
                }
            }
            'u' => {
                if magic(i) {
                    "juː"
                } else {
                    "ʌ"
                }
            }
            'y' => {
                if i == 0 {
                    "j"
                } else if i + 1 == n {
                    if n <= 3 {
                        "I"
                    } else {
                        "i"
                    }
                } else {
                    "ɪ"
                }
            }
            'c' => {
                if "eiy".contains(at(i + 1)) {
                    "s"
                } else {
                    "k"
                }
            }
            'g' => {
                if "ei".contains(at(i + 1)) && i + 2 == n {
                    "ʤ"
                } else {
                    "ɡ"
                }
            }
            'j' => "ʤ",
            'r' => "ɹ",
            'x' => {
                if i == 0 {
                    "z"
                } else {
                    "ks"
                }
            }
            'q' => "k",
            's' => {
                if i > 0 && i + 1 == n && !"sptkf".contains(at(i - 1)) {
                    "z"
                } else {
                    "s"
                }
            }
            c => {
                out.push(c);
                ""
            }
        };
        out.push_str(p);
        // a doubled consonant is one sound
        if i + 1 < n && at(i + 1) == c && !vowel(c) {
            i += 1;
        }
        i += 1;
    }
    restress(&out, PRIMARY)
}

/// Maps espeak-ng's IPA onto Kokoro's British phonemes (misaki's alphabet).
fn from_espeak(ipa: &str) -> String {
    let mut s = ipa.trim().to_string();
    #[rustfmt::skip]
    let pairs = [
        ("eɪ", "A"), ("aɪ", "I"), ("əʊ", "Q"), ("aʊ", "W"), ("ɔɪ", "Y"), ("dʒ", "ʤ"), ("tʃ", "ʧ"), ("eə", "ɛː"),
        ("r", "ɹ"), ("ɐ", "ə"), ("ᵻ", "ɪ"), ("ɾ", "t"), ("ʔ", "t"), ("x", "k"), ("ɬ", "l"), ("e", "ɛ"), ("ɚ", "ə"),
        ("ɝ", "ɜː"), ("g", "ɡ"), ("o", "ɔ"), ("ɑ̃", "ɒ"),
    ];
    for (a, b) in pairs {
        s = s.replace(a, b);
    }
    s.chars()
        .filter(|c| GB_VOCAB.contains(*c) || *c == ' ')
        .collect()
}

/// Phonemes for words the lexicons don't know.
pub struct Fallback {
    espeak: Option<std::path::PathBuf>,
    cache: Mutex<HashMap<String, String>>,
}

impl Fallback {
    /// Finds `espeak-ng` on PATH and in Homebrew's prefixes.
    pub fn new() -> Fallback {
        let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
        let espeak = dirs
            .into_iter()
            .map(|d| d.join("espeak-ng"))
            .find(|p| p.is_file());
        Fallback {
            espeak,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// No espeak-ng, only the spelling rules (tests).
    pub fn rules_only() -> Fallback {
        Fallback {
            espeak: None,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn has_espeak(&self) -> bool {
        self.espeak.is_some()
    }

    fn word(&self, w: &str) -> String {
        if let Some(p) = self.cache.lock().ok().and_then(|c| c.get(w).cloned()) {
            return p;
        }
        let ps = self
            .espeak
            .as_ref()
            .and_then(|bin| {
                let out = std::process::Command::new(bin)
                    .args(["-q", "--ipa", "-v", "en-gb", w])
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()?;
                let p = from_espeak(&String::from_utf8_lossy(&out.stdout));
                (out.status.success() && p.chars().any(is_vowel)).then_some(p)
            })
            .unwrap_or_else(|| spelling_rules(w));
        if let Ok(mut c) = self.cache.lock() {
            c.insert(w.to_string(), ps.clone());
        }
        ps
    }
}

impl Default for Fallback {
    fn default() -> Self {
        Fallback::new()
    }
}

/// English text to Kokoro phonemes.
pub struct G2p {
    pub lex: Lexicon,
    pub fallback: Fallback,
}

/// What follows a word: a vowel, a consonant, or a pause (punctuation or the end).
#[derive(Clone, Copy, PartialEq)]
enum Next {
    Vowel,
    Consonant,
    Pause,
}

impl G2p {
    pub fn new(lex: Lexicon, fallback: Fallback) -> G2p {
        G2p { lex, fallback }
    }

    /// Loads a compiled lexicon from disk.
    pub fn load(lexicon: &Path) -> std::io::Result<G2p> {
        Ok(G2p::new(
            Lexicon::from_compiled(std::fs::read_to_string(lexicon)?),
            Fallback::new(),
        ))
    }

    /// Letter names: "CI" is "sˌiːˈI".
    fn spell(&self, w: &str) -> Option<String> {
        let mut ps = String::new();
        for c in w.chars().filter(|c| c.is_alphabetic()) {
            ps.push_str(self.lex.get(&c.to_uppercase().to_string())?.0);
        }
        let ps = apply_stress(&ps, 0.0);
        Some(match ps.rfind(SECONDARY) {
            Some(i) => format!("{}{PRIMARY}{}", &ps[..i], &ps[i + SECONDARY.len_utf8()..]),
            None => ps,
        })
    }

    fn special(w: &str, next: Next) -> Option<&'static str> {
        Some(match w {
            "a" | "A" => "ɐ",
            "an" | "An" => "ɐn",
            "I" => "ˌI",
            "the" | "The" => {
                if next == Next::Vowel {
                    "ði"
                } else {
                    "ðə"
                }
            }
            "to" | "To" => match next {
                Next::Vowel => "tʊ",
                Next::Consonant => "tə",
                Next::Pause => "tˈuː",
            },
            "in" | "In" => {
                if next == Next::Pause {
                    "ˈɪn"
                } else {
                    "ɪn"
                }
            }
            _ => return None,
        })
    }

    /// The lexicon, with the -s, -ed and -ing endings misaki derives.
    fn lexical(&self, w: &str, next: Next) -> Option<String> {
        if let Some((ps, end)) = self.lex.find(w) {
            return Some(
                if next == Next::Pause {
                    end.unwrap_or(ps)
                } else {
                    ps
                }
                .to_string(),
            );
        }
        let n = w.len();
        if !w.is_ascii() {
            return None;
        }
        if let Some(stem) = w.strip_suffix("'s").or_else(|| w.strip_suffix("s'")) {
            return self.lexical(stem, Next::Consonant).map(|p| plural_s(&p));
        }
        if n >= 3 && w.ends_with('s') {
            let stem = if !w.ends_with("ss") && self.lex.known(&w[..n - 1]) {
                Some(w[..n - 1].to_string())
            } else if n > 4
                && w.ends_with("es")
                && !w.ends_with("ies")
                && self.lex.known(&w[..n - 2])
            {
                Some(w[..n - 2].to_string())
            } else if n > 4 && w.ends_with("ies") && self.lex.known(&format!("{}y", &w[..n - 3])) {
                Some(format!("{}y", &w[..n - 3]))
            } else {
                None
            };
            if let Some(s) = stem {
                return self.lex.find(&s).map(|(p, _)| plural_s(p));
            }
        }
        if n >= 4 && w.ends_with('d') {
            let stem = if !w.ends_with("dd") && self.lex.known(&w[..n - 1]) {
                Some(&w[..n - 1])
            } else if n > 4
                && w.ends_with("ed")
                && !w.ends_with("eed")
                && self.lex.known(&w[..n - 2])
            {
                Some(&w[..n - 2])
            } else {
                None
            };
            if let Some(s) = stem {
                return self.lex.find(s).map(|(p, _)| past_ed(p));
            }
        }
        if n >= 5 && w.ends_with("ing") {
            let base = &w[..n - 3];
            let b: Vec<char> = base.chars().collect();
            let stem = if n > 5 && self.lex.known(base) {
                Some(base.to_string())
            } else if self.lex.known(&format!("{base}e")) {
                Some(format!("{base}e"))
            } else if b.len() > 2
                && b[b.len() - 1] == b[b.len() - 2]
                && self.lex.known(&base[..base.len() - 1])
            {
                Some(base[..base.len() - 1].to_string())
            } else {
                None
            };
            if let Some(s) = stem {
                let p = self.lex.find(&s).map(|(p, _)| p.to_string())?;
                if p.ends_with(['ə', 'ː']) {
                    return None;
                }
                return Some(format!("{}ɪŋ", apply_stress(&p, 0.5)));
            }
        }
        None
    }

    /// One word's phonemes.
    fn word(&self, w: &str, next: Next) -> String {
        if let Some(p) = Self::special(w, next) {
            return p.to_string();
        }
        if let Some(p) = self.lexical(w, next) {
            // capitalised words carry at least secondary stress, as in misaki
            let stress = if w.chars().all(|c| !c.is_lowercase()) && w.len() > 1 {
                2.0
            } else if w.starts_with(char::is_uppercase) {
                0.5
            } else {
                0.0
            };
            return if stress > 0.0 {
                apply_stress(&p, stress)
            } else {
                p
            };
        }
        // initialisms: CI, PR, API
        if w.len() <= 5 && w.chars().all(|c| c.is_ascii_uppercase()) {
            if let Some(p) = self.spell(w) {
                return p;
            }
        }
        // camelCase and PascalCase: GitHub, iPhone
        let cs: Vec<char> = w.chars().collect();
        if let Some(k) = (1..cs.len()).find(|&k| cs[k - 1].is_lowercase() && cs[k].is_uppercase()) {
            let (a, b): (String, String) = (cs[..k].iter().collect(), cs[k..].iter().collect());
            return format!(
                "{} {}",
                self.word(&a, Next::Consonant),
                apply_stress(&self.word(&b, next), -1.0)
            );
        }
        // compounds of two known words: "codebase", "dashboard"
        let lower = w.to_lowercase();
        if lower.is_ascii() && lower.len() >= 6 {
            for k in (3..=lower.len() - 3).rev() {
                let (a, b) = lower.split_at(k);
                if let (Some(pa), Some(pb)) =
                    (self.lexical(a, Next::Consonant), self.lexical(b, next))
                {
                    return format!("{pa}{}", apply_stress(&pb, -1.0));
                }
            }
        }
        // short consonant runs are letters: "npm", "SQL"
        if w.len() <= 4 && !lower.chars().any(|c| "aeiouy".contains(c)) {
            if let Some(p) = self.spell(w) {
                return p;
            }
        }
        self.fallback.word(&lower)
    }

    /// Text to phonemes: words separated by spaces, punctuation kept for Kokoro's pauses.
    pub fn phonemize(&self, text: &str) -> String {
        let toks = text::normalise(text);
        let mut out: Vec<String> = Vec::with_capacity(toks.len());
        let mut next = Next::Pause;
        for t in toks.iter().rev() {
            match t {
                Tok::Punct(p) => {
                    out.push(p.to_string());
                    next = Next::Pause;
                }
                Tok::Word(w) => {
                    let ps = self.word(w, next);
                    next = match ps.chars().find(|c| is_vowel(*c) || CONSONANTS.contains(*c)) {
                        Some(c) if is_vowel(c) => Next::Vowel,
                        Some(_) => Next::Consonant,
                        None => next,
                    };
                    out.push(ps);
                }
            }
        }
        out.reverse();
        let mut s = String::new();
        for (i, (p, t)) in out.iter().zip(&toks).enumerate() {
            if i > 0 && matches!(t, Tok::Word(_)) {
                s.push(' ');
            }
            s.push_str(p);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g2p() -> G2p {
        let gold = r#"{"good":"ɡˈʊd","evening":"ˈiːvnɪŋ","sir":"sˌɜː","three":"θɹˈiː","agent":"ˈAʤᵊnt","run":"ɹˈʌn","would":{"DEFAULT":"wʊd","None":"wˈʊd"},
            "the":"ðə","pass":"pˈɑːs","test":"tˈɛst","C":"sˈiː","I":"ˈI","P":"pˈiː","R":"ˈɑː","code":"kˈQd","base":"bˈAs","git":"ɡˈɪt","hub":"hˈʌb",
            "and":"and","one":"wˈʌn","dollar":"dˈɒlə","it":"ɪt","merge":"mˈɜːʤ","touch":"tˈʌʧ"}"#;
        let silver = r#"{"tests":"tˈɛsts","good":"ɡˈʊʊd"}"#;
        G2p::new(
            Lexicon::from_compiled(Lexicon::compile(gold, silver).unwrap()),
            Fallback::rules_only(),
        )
    }

    #[test]
    fn lexicon_compiles_sorted_and_gold_wins() {
        let g = g2p();
        assert_eq!(g.lex.get("good"), Some(("ɡˈʊd", None)));
        assert_eq!(g.lex.get("would"), Some(("wʊd", Some("wˈʊd"))));
        assert_eq!(g.lex.get("nope"), None);
        assert_eq!(g.lex.find("Good"), Some(("ɡˈʊd", None)));
    }

    #[test]
    fn sentences_get_phonemes_and_punctuation() {
        let g = g2p();
        assert_eq!(g.phonemize("Good evening, sir."), "ɡˈʊd ˈiːvnɪŋ, sˌɜː.");
        assert_eq!(g.phonemize("I would."), "ˌI wˈʊd.");
        assert_eq!(g.phonemize("I would run"), "ˌI wʊd ɹˈʌn");
    }

    #[test]
    fn stems_initialisms_and_compounds() {
        let g = g2p();
        assert_eq!(g.phonemize("agents"), "ˈAʤᵊnts");
        assert_eq!(g.phonemize("passed"), "pˈɑːst");
        assert_eq!(g.phonemize("merged"), "mˈɜːʤd");
        assert_eq!(g.phonemize("touches"), "tˈʌʧɪz");
        assert_eq!(g.phonemize("running"), "ɹˈʌnɪŋ");
        assert_eq!(g.phonemize("CI"), "sˌiːˈI");
        assert_eq!(g.phonemize("codebase"), "kˈQdbˌAs");
        assert_eq!(g.phonemize("GitHub"), "ɡˈɪt hˌʌb");
        assert_eq!(g.phonemize("the agent"), "ði ˈAʤᵊnt");
        assert_eq!(g.phonemize("$1"), "wˈʌn dˈɒlə");
    }

    #[test]
    fn unknown_words_still_speak() {
        let g = g2p();
        for w in ["kokoro", "sphere", "zyxt", "ratatouille", "blorp"] {
            let p = g.phonemize(w);
            assert!(p.chars().any(is_vowel), "{w} -> {p}");
            assert!(
                p.chars().all(|c| GB_VOCAB.contains(c) || c == ' '),
                "{w} -> {p}"
            );
        }
        assert_eq!(spelling_rules("blorp"), "blˈɔːp");
        assert_eq!(from_espeak("həlˈəʊ wˈɜːld"), "həlˈQ wˈɜːld");
    }
}
