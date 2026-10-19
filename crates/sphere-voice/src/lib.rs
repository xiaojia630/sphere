//! Sphere's spoken voice: Kokoro-82M (hexgrad, Apache-2.0) with the British `bm_george` voice, run through ONNX
//! Runtime on the CPU. Text becomes phonemes with misaki's British lexicons (`g2p`), is cut into sentences, and each
//! sentence is synthesised and handed on as soon as it is ready, so the first one plays while the rest are made.
//!
//! The model (kokoro-onnx's int8 export) and the lexicons are downloaded once into the app's data folder and checked
//! against their SHA-256; the bm_george style tensor (510 x 256 f32) is embedded. Until everything is loaded, or if
//! anything fails, `Speaker::speak` returns false and the caller uses the system voice instead.
pub mod g2p;
pub mod text;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Instant;

pub use g2p::G2p;

/// Output sample rate (mono).
pub const SAMPLE_RATE: u32 = 24_000;
/// Gabriel's choice: George, a touch slower than the model's default.
pub const SPEED: f32 = 0.95;
/// Kokoro takes at most 510 phoneme tokens per pass (512 with the pads).
const MAX_TOKENS: usize = 510;

const STYLE: &[u8] = include_bytes!("../assets/bm_george.f32");
const STYLE_DIM: usize = 256;

/// A file the voice needs, where it comes from and its SHA-256.
pub struct Asset {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

pub const MODEL: Asset = Asset {
    name: "kokoro-v1.0.int8.onnx",
    url: "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.int8.onnx",
    sha256: "6e742170d309016e5891a994e1ce1559c702a2ccd0075e67ef7157974f6406cb",
    size: 92_361_271,
};
pub const GOLD: Asset = Asset {
    name: "gb_gold.json",
    url: "https://raw.githubusercontent.com/hexgrad/misaki/fba1236595f2d2bf21d414ba6e57d25256afada3/misaki/data/gb_gold.json",
    sha256: "29e62f4b60261c88f7f3c2c7811ca3825978948090b72d2b27d565b729282f71",
    size: 2_838_552,
};
pub const SILVER: Asset = Asset {
    name: "gb_silver.json",
    url: "https://raw.githubusercontent.com/hexgrad/misaki/fba1236595f2d2bf21d414ba6e57d25256afada3/misaki/data/gb_silver.json",
    sha256: "48131e2d92ccc41655f4543e87e0f938e71463eb5a54be7f0693bb712ebb6bce",
    size: 3_663_898,
};
/// The compiled lexicon, named for the misaki revision it came from.
const LEXICON: &str = "lexicon-gb-fba1236.tsv";

/// Where the voice files live: ~/Library/Application Support/Sphere/voice on macOS.
pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Sphere/voice")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("sphere/voice")
    }
}

fn sha256_file(p: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Downloads `a` into `dir` with curl unless it is already there, and checks its SHA-256.
fn fetch(dir: &Path, a: &Asset) -> Result<PathBuf, String> {
    let dest = dir.join(a.name);
    if std::fs::metadata(&dest).is_ok_and(|m| m.len() == a.size) {
        return Ok(dest);
    }
    let part = dir.join(format!("{}.part", a.name));
    let ok = std::process::Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--connect-timeout", "20", "-o"])
        .arg(&part)
        .arg(a.url)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("curl: {e}"))?
        .success();
    if !ok {
        let _ = std::fs::remove_file(&part);
        return Err(format!("download of {} failed", a.name));
    }
    let got = sha256_file(&part).map_err(|e| e.to_string())?;
    if got != a.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "{} has SHA-256 {got}, expected {}",
            a.name, a.sha256
        ));
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Makes sure the model and the compiled lexicon are in `dir`, downloading them the first time.
/// Returns (model, lexicon).
pub fn ensure_files(dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let lexicon = dir.join(LEXICON);
    if !lexicon.is_file() {
        let gold = fetch(dir, &GOLD)?;
        let silver = fetch(dir, &SILVER)?;
        let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| e.to_string());
        let compiled = g2p::Lexicon::compile(&read(&gold)?, &read(&silver)?)?;
        let tmp = dir.join(format!("{LEXICON}.part"));
        std::fs::write(&tmp, compiled).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &lexicon).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(gold);
        let _ = std::fs::remove_file(silver);
    }
    let model = fetch(dir, &MODEL)?;
    Ok((model, lexicon))
}

/// Kokoro's phoneme vocabulary (kokoro-onnx's config.json).
#[rustfmt::skip]
fn token_id(c: char) -> Option<i64> {
    Some(match c {
        ';' => 1, ':' => 2, ',' => 3, '.' => 4, '!' => 5, '?' => 6, '—' => 9, '…' => 10, '"' => 11, '(' => 12, ')' => 13,
        '“' => 14, '”' => 15, ' ' => 16, '\u{303}' => 17, 'ʣ' => 18, 'ʥ' => 19, 'ʦ' => 20, 'ʨ' => 21, 'ᵝ' => 22,
        '\u{AB67}' => 23, 'A' => 24, 'I' => 25, 'O' => 31, 'Q' => 33, 'S' => 35, 'T' => 36, 'W' => 39, 'Y' => 41,
        'ᵊ' => 42, 'a' => 43, 'b' => 44, 'c' => 45, 'd' => 46, 'e' => 47, 'f' => 48, 'h' => 50, 'i' => 51, 'j' => 52,
        'k' => 53, 'l' => 54, 'm' => 55, 'n' => 56, 'o' => 57, 'p' => 58, 'q' => 59, 'r' => 60, 's' => 61, 't' => 62,
        'u' => 63, 'v' => 64, 'w' => 65, 'x' => 66, 'y' => 67, 'z' => 68, 'ɑ' => 69, 'ɐ' => 70, 'ɒ' => 71, 'æ' => 72,
        'β' => 75, 'ɔ' => 76, 'ɕ' => 77, 'ç' => 78, 'ɖ' => 80, 'ð' => 81, 'ʤ' => 82, 'ə' => 83, 'ɚ' => 85, 'ɛ' => 86,
        'ɜ' => 87, 'ɟ' => 90, 'ɡ' => 92, 'ɥ' => 99, 'ɨ' => 101, 'ɪ' => 102, 'ʝ' => 103, 'ɯ' => 110, 'ɰ' => 111,
        'ŋ' => 112, 'ɳ' => 113, 'ɲ' => 114, 'ɴ' => 115, 'ø' => 116, 'ɸ' => 118, 'θ' => 119, 'œ' => 120, 'ɹ' => 123,
        'ɾ' => 125, 'ɻ' => 126, 'ʁ' => 128, 'ɽ' => 129, 'ʂ' => 130, 'ʃ' => 131, 'ʈ' => 132, 'ʧ' => 133, 'ʊ' => 135,
        'ʋ' => 136, 'ʌ' => 138, 'ɣ' => 139, 'ɤ' => 140, 'χ' => 142, 'ʎ' => 143, 'ʒ' => 147, 'ʔ' => 148, 'ˈ' => 156,
        'ˌ' => 157, 'ː' => 158, 'ʰ' => 162, 'ʲ' => 164, '↓' => 169, '→' => 171, '↗' => 172, '↘' => 173, 'ᵻ' => 177,
        _ => return None,
    })
}

pub fn tokenize(phonemes: &str) -> Vec<i64> {
    phonemes.chars().filter_map(token_id).collect()
}

/// One piece of speech to synthesise, and the pause after it in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub phonemes: String,
    pub pause: f32,
}

/// Longest piece synthesised at once: memory grows with the audio a pass makes, so long sentences go in parts.
const MAX_PIECE: usize = 80;
/// The first piece of an utterance is kept about this short so its audio comes quickly.
const FIRST_PIECE: usize = 20;
const SENTENCE_PAUSE: f32 = 0.22;
const CLAUSE_PAUSE: f32 = 0.06;

/// Where a piece may end: after a clause mark or between words.
struct Cut {
    at: usize,
    tokens: usize,
    clause: bool,
}

fn cuts(s: &str) -> Vec<Cut> {
    let mut out = Vec::new();
    let mut tokens = 0;
    let mut prev = ' ';
    for (i, c) in s.char_indices() {
        if c == ' ' && i > 0 {
            out.push(Cut {
                at: i,
                tokens,
                clause: matches!(prev, ',' | ';' | ':' | '—'),
            });
        }
        tokens += usize::from(token_id(c).is_some() && c != ' ');
        prev = c;
    }
    out
}

/// Cuts one sentence into pieces of at most `max` tokens, at clauses when it can, else between words; the first
/// piece is cut at `first` tokens or fewer when `first` is given.
fn cut_sentence(s: &str, first: Option<usize>, out: &mut Vec<Piece>) {
    let mut rest = s.trim();
    let mut limit = first.unwrap_or(MAX_PIECE);
    loop {
        let n = tokenize(rest).iter().filter(|t| **t != 16).count();
        if n <= limit {
            out.push(Piece {
                phonemes: rest.to_string(),
                pause: SENTENCE_PAUSE,
            });
            return;
        }
        let cs = cuts(rest);
        // the last clause that fits, if it isn't tiny (a first clause may run half as long again: it sounds better
        // than a cut between words); else the last word that fits
        let clause_limit = if first.is_some() && out.is_empty() {
            limit * 3 / 2
        } else {
            limit
        };
        let clause = cs
            .iter()
            .rfind(|c| c.clause && c.tokens <= clause_limit && c.tokens >= limit / 3);
        let word = cs.iter().rfind(|c| c.tokens <= limit && c.tokens >= 4);
        let Some(c) = clause.or(word).or(cs.first()) else {
            out.push(Piece {
                phonemes: rest.to_string(),
                pause: SENTENCE_PAUSE,
            });
            return;
        };
        out.push(Piece {
            phonemes: rest[..c.at].to_string(),
            pause: if c.clause { CLAUSE_PAUSE } else { 0.0 },
        });
        rest = rest[c.at..].trim();
        limit = MAX_PIECE;
    }
}

/// Cuts phonemes into sentences, and sentences longer than `MAX_PIECE` tokens at clauses (then words). The first
/// piece is kept to about `FIRST_PIECE` tokens so the first audio comes quickly.
pub fn split(phonemes: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut start = 0;
    let cs: Vec<(usize, char)> = phonemes.char_indices().collect();
    for (k, &(i, c)) in cs.iter().enumerate() {
        let end = k + 1 == cs.len();
        if (matches!(c, '.' | '!' | '?' | '…') && cs.get(k + 1).is_none_or(|(_, n)| *n == ' '))
            || end
        {
            let e = i + c.len_utf8();
            let sentence = phonemes[start..e].trim();
            if sentence
                .chars()
                .any(|c| token_id(c).is_some_and(|t| t > 16))
            {
                cut_sentence(sentence, out.is_empty().then_some(FIRST_PIECE), &mut out);
            }
            start = e;
        }
    }
    if let Some(l) = out.last_mut() {
        l.pause = 0.0;
    }
    out
}

/// Cuts the silence the model leaves at either end (frames 48 dB below the loudest), keeping a few ms.
pub fn trim(audio: &[f32]) -> &[f32] {
    const FRAME: usize = 240;
    let rms: Vec<f32> = audio
        .chunks(FRAME)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let peak = rms.iter().cloned().fold(0.0, f32::max);
    if peak <= 0.0 {
        return &audio[..0];
    }
    let floor = peak * 10f32.powf(-48.0 / 20.0);
    let first = rms.iter().position(|r| *r > floor).unwrap_or(0);
    let last = rms
        .iter()
        .rposition(|r| *r > floor)
        .unwrap_or(rms.len() - 1);
    let a = first.saturating_sub(1) * FRAME;
    let b = ((last + 2) * FRAME).min(audio.len());
    &audio[a..b]
}

/// Kokoro in an ONNX Runtime session.
pub struct Engine {
    session: ort::session::Session,
    ids_input: String,
    style: Vec<f32>,
}

impl Engine {
    /// Loads the model with `threads` intra-op threads.
    pub fn load(model: &Path, threads: usize) -> Result<Engine, String> {
        let e = |e: ort::Error| e.to_string();
        // ONNX Runtime allows one environment per process
        static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> =
            std::sync::OnceLock::new();
        let env = ENV
            .get_or_init(|| {
                ort::init()
                    .with_name("sphere-voice")
                    .with_telemetry(false)
                    .build()
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|e| e.clone())?;
        let session = ort::session::Session::builder(env)
            .map_err(e)?
            .with_intra_threads(threads)
            .map_err(|x| x.to_string())?
            .with_inter_threads(1)
            .map_err(|x| x.to_string())?
            // ort picks CoreML on macOS by default; Sphere's voice stays on the CPU
            .with_auto_device(ort::session::builder::AutoDevicePolicy::PreferCPU)
            .map_err(|x| x.to_string())?
            // the arena reuses the large activation buffers between passes; without it the footprint is ~60% higher
            .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(true).build()])
            .map_err(|x| x.to_string())?
            .with_memory_pattern(true)
            .map_err(|x| x.to_string())?
            .commit_from_file(model)
            .map_err(e)?;
        let ids_input = session
            .inputs()
            .iter()
            .map(|i| i.name().to_string())
            .find(|n| n == "input_ids" || n == "tokens")
            .ok_or("model has no token input")?;
        let style = STYLE
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        Ok(Engine {
            session,
            ids_input,
            style,
        })
    }

    /// Synthesises up to 510 phoneme tokens at `speed`, returning 24 kHz mono samples.
    pub fn synth(&mut self, phonemes: &str, speed: f32) -> Result<Vec<f32>, String> {
        let mut ids = tokenize(phonemes);
        ids.truncate(MAX_TOKENS);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // the style vector is chosen by the number of tokens
        let row = ids.len().min(self.style.len() / STYLE_DIM) - 1;
        let style = self.style[row * STYLE_DIM..(row + 1) * STYLE_DIM].to_vec();
        let n = ids.len() + 2;
        let mut padded = Vec::with_capacity(n);
        padded.push(0);
        padded.extend(ids);
        padded.push(0);
        let e = |e: ort::Error| e.to_string();
        let inputs = ort::inputs![
            self.ids_input.as_str() => ort::value::Tensor::from_array(([1usize, n], padded)).map_err(e)?,
            "style" => ort::value::Tensor::from_array(([1usize, STYLE_DIM], style)).map_err(e)?,
            "speed" => ort::value::Tensor::from_array(([1usize], vec![speed])).map_err(e)?,
        ];
        let out = self.session.run(inputs).map_err(e)?;
        let (_, audio) = out[0].try_extract_tensor::<f32>().map_err(e)?;
        Ok(audio.to_vec())
    }
}

/// Speech for the playback thread: samples at `SAMPLE_RATE` for utterance `gen`.
pub struct Chunk {
    pub gen: u64,
    pub samples: Vec<f32>,
    /// the first chunk of its utterance
    pub first: bool,
}

/// After this long without speech the model is unloaded (its ~300 MB freed); the next line reloads it (~0.4 s).
const IDLE_UNLOAD: std::time::Duration = std::time::Duration::from_secs(600);

const LOADING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// The voice on its own thread. `speak` queues an utterance and returns at once; chunks go to the sink as they are
/// made. A newer `speak` or a `stop` abandons what is left of the previous one.
pub struct Speaker {
    tx: mpsc::Sender<(u64, String)>,
    gen: Arc<AtomicU64>,
    state: Arc<AtomicU8>,
}

impl Speaker {
    /// Starts the voice thread: it fetches the files if needed, loads them, warms the model up, then waits for text.
    pub fn start(dir: PathBuf, sink: impl Fn(Chunk) + Send + 'static) -> Speaker {
        let (tx, rx) = mpsc::channel::<(u64, String)>();
        let gen = Arc::new(AtomicU64::new(0));
        let state = Arc::new(AtomicU8::new(LOADING));
        let (g, st) = (gen.clone(), state.clone());
        let spawned = std::thread::Builder::new()
            .name("sphere-voice".into())
            .spawn(move || {
                let loaded = (|| -> Result<(PathBuf, Engine, G2p), String> {
                    let (model, lexicon) = ensure_files(&dir)?;
                    let g2p = G2p::load(&lexicon).map_err(|e| e.to_string())?;
                    let mut engine = Engine::load(&model, threads())?;
                    engine.synth(&g2p.phonemize("Ready."), SPEED)?;
                    Ok((model, engine, g2p))
                })();
                let (model, engine, g2p) = match loaded {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("sphere: voice unavailable, using the system voice ({e})");
                        st.store(FAILED, Ordering::SeqCst);
                        return;
                    }
                };
                let mut engine = Some(engine);
                st.store(READY, Ordering::SeqCst);
                loop {
                    let mut job = match rx.recv_timeout(IDLE_UNLOAD) {
                        Ok(j) => j,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // quiet for a while: give the model's memory back until the next line
                            engine = None;
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    };
                    // only the newest request matters
                    while let Ok(j) = rx.try_recv() {
                        job = j;
                    }
                    let (id, text) = job;
                    if g.load(Ordering::SeqCst) != id {
                        continue;
                    }
                    if engine.is_none() {
                        match Engine::load(&model, threads()) {
                            Ok(e) => engine = Some(e),
                            Err(e) => {
                                eprintln!("sphere: voice: {e}");
                                continue;
                            }
                        }
                    }
                    let Some(eng) = engine.as_mut() else { continue };
                    let mut first = true;
                    for piece in split(&g2p.phonemize(&text)) {
                        if g.load(Ordering::SeqCst) != id {
                            break;
                        }
                        let audio = match eng.synth(&piece.phonemes, SPEED) {
                            Ok(a) => a,
                            Err(e) => {
                                eprintln!("sphere: voice: {e}");
                                break;
                            }
                        };
                        let mut samples = trim(&audio).to_vec();
                        samples.extend(std::iter::repeat_n(
                            0.0,
                            (piece.pause * SAMPLE_RATE as f32) as usize,
                        ));
                        if g.load(Ordering::SeqCst) != id {
                            break;
                        }
                        sink(Chunk {
                            gen: id,
                            samples,
                            first,
                        });
                        first = false;
                    }
                }
            });
        if spawned.is_err() {
            state.store(FAILED, Ordering::SeqCst);
        }
        Speaker { tx, gen, state }
    }

    /// True once the model is loaded and warm.
    pub fn ready(&self) -> bool {
        self.state.load(Ordering::SeqCst) == READY
    }

    pub fn failed(&self) -> bool {
        self.state.load(Ordering::SeqCst) == FAILED
    }

    /// The current utterance number; chunks with another number are stale.
    pub fn current(&self) -> u64 {
        self.gen.load(Ordering::SeqCst)
    }

    /// Queues `text` and returns its utterance number, or None when the voice isn't ready (use the system voice).
    pub fn speak(&self, text: &str) -> Option<u64> {
        if !self.ready() {
            return None;
        }
        let id = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.tx.send((id, text.to_string())).ok()?;
        Some(id)
    }

    /// Abandons the current utterance; returns the new utterance number (nothing is playing under it).
    pub fn stop(&self) -> u64 {
        self.gen.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// Inference threads: half the cores, between 2 and 4, so the voice never starves the frame.
fn threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 4))
}

/// Times a full render (for the example and the numbers in the PR).
pub fn render(engine: &mut Engine, g2p: &G2p, text: &str) -> Result<(Vec<f32>, f32, f32), String> {
    let t = Instant::now();
    let mut out = Vec::new();
    let mut first = None;
    for piece in split(&g2p.phonemize(text)) {
        let audio = engine.synth(&piece.phonemes, SPEED)?;
        out.extend_from_slice(trim(&audio));
        first.get_or_insert(t.elapsed().as_secs_f32());
        out.extend(std::iter::repeat_n(
            0.0,
            (piece.pause * SAMPLE_RATE as f32) as usize,
        ));
    }
    Ok((out, first.unwrap_or(0.0), t.elapsed().as_secs_f32()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_tensor_is_george_shaped() {
        assert_eq!(STYLE.len(), 510 * STYLE_DIM * 4);
        let first = f32::from_le_bytes([STYLE[0], STYLE[1], STYLE[2], STYLE[3]]);
        assert!(first.is_finite());
    }

    #[test]
    fn tokens_follow_kokoro_vocab() {
        assert_eq!(
            tokenize("ɡˈʊd, sˌɜː."),
            vec![92, 156, 135, 46, 3, 16, 61, 157, 87, 158, 4]
        );
        assert_eq!(tokenize("❓"), Vec::<i64>::new());
    }

    #[test]
    fn split_keeps_sentences_and_a_short_first_piece() {
        let p = split("ɡˈʊd ˈiːvnɪŋ, sˌɜː. θɹˈiː ˈAʤᵊnts ɑː ɹˈʌnɪŋ.");
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].phonemes, "ɡˈʊd ˈiːvnɪŋ, sˌɜː.");
        assert!(p[0].pause > 0.1 && p[1].pause == 0.0);
        // a long first sentence is cut at a clause, a long later one at clauses and words, nothing is lost
        let long = "ðə pˈAmᵊnts tˈɛsts hav pˈɑːst, and wˈʌn ˈAʤᵊnt wʊd lˈIk jɔː pəmˈɪʃᵊn bɪfˈɔː ɪt tˈʌʧɪz ðə dˈAtəbAs.";
        let p = split(long);
        assert_eq!(p[0].phonemes, "ðə pˈAmᵊnts tˈɛsts hav pˈɑːst,");
        assert_eq!(
            p.iter()
                .map(|p| p.phonemes.clone())
                .collect::<Vec<_>>()
                .join(" "),
            long
        );
        let p = split(&format!("ɡˈʊd. {long} {long}"));
        assert_eq!(p[0].phonemes, "ɡˈʊd.");
        assert!(p
            .iter()
            .all(|p| tokenize(&p.phonemes).len() <= MAX_PIECE + 20));
        // with no clause to cut at, the first piece ends between words
        let p = split(&"ɡˈʊd ".repeat(40));
        assert!(tokenize(&p[0].phonemes).len() <= FIRST_PIECE + 5 && p.len() > 2);
        assert!(split(&"ɡˈʊd ".repeat(300))
            .iter()
            .all(|p| tokenize(&p.phonemes).len() <= MAX_PIECE + 20));
        assert!(split(" . ").is_empty());
    }

    /// Streams and cancels with the real model; skipped when it hasn't been downloaded (tests never download).
    #[test]
    fn speaker_streams_in_order_and_stops() {
        let dir = data_dir();
        if !dir.join(MODEL.name).is_file() || !dir.join(LEXICON).is_file() {
            eprintln!("skipped: no voice files in {}", dir.display());
            return;
        }
        let (tx, rx) = mpsc::channel();
        let sp = Speaker::start(dir, move |c| {
            let _ = tx.send((c.gen, c.first, c.samples.len()));
        });
        assert_eq!(sp.speak("Not yet."), None);
        while !sp.ready() {
            assert!(!sp.failed());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let a = sp.speak("One. Two. Three. Four. Five. Six.").unwrap();
        let (gen, first, n) = rx.recv().unwrap();
        assert!(gen == a && first && n > SAMPLE_RATE as usize / 10);
        // a new line abandons the rest of the old one
        let b = sp.speak("Seven.").unwrap();
        let mut got = Vec::new();
        while let Ok(c) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
            got.push(c);
        }
        let from_b: Vec<_> = got.iter().filter(|c| c.0 == b).collect();
        assert!(from_b.len() == 1 && from_b[0].1);
        assert!(got.iter().filter(|c| c.0 == a).count() <= 1);
        sp.stop();
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err());
    }

    #[test]
    fn trim_cuts_silence() {
        let mut a = vec![0.0; 4800];
        a.extend((0..2400).map(|i| (i as f32 * 0.1).sin() * 0.5));
        a.extend(vec![0.0; 4800]);
        let t = trim(&a);
        assert!(t.len() < 2400 + 4 * 240 && t.len() >= 2400);
    }
}
