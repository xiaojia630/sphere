//! Renders text with Sphere's voice to a WAV and prints the timings:
//! `cargo run --release -p sphere-voice --example say -- out.wav "Good evening, sir."`
//! Add `--phonemes` to print the phonemes. Downloads the model on first use, like the app.
use sphere_voice::*;
use std::time::Instant;

/// Physical footprint in MB (what Activity Monitor shows as Memory), from macOS's `footprint`; RSS elsewhere.
fn rss_mb() -> f64 {
    let pid = std::process::id().to_string();
    if let Ok(out) = std::process::Command::new("footprint")
        .args(["-p", &pid])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(l) = text.lines().find(|l| l.contains("Footprint:")) {
            let mut it = l
                .split_whitespace()
                .skip_while(|w| *w != "Footprint:")
                .skip(1);
            if let (Some(v), Some(u)) = (it.next().and_then(|v| v.parse::<f64>().ok()), it.next()) {
                return match u {
                    "KB" => v / 1024.0,
                    "GB" => v * 1024.0,
                    _ => v,
                };
            }
        }
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let show = args.iter().any(|a| a == "--phonemes");
    let args: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let (Some(out), Some(text)) = (args.first(), args.get(1)) else {
        eprintln!("usage: say OUT.wav TEXT [--phonemes]");
        std::process::exit(2);
    };
    let base = rss_mb();
    let t = Instant::now();
    let (model, lexicon) = ensure_files(&data_dir()).expect("voice files");
    let files = t.elapsed();
    let t = Instant::now();
    let g2p = G2p::load(&lexicon).unwrap();
    let lex_t = t.elapsed();
    let lex_rss = rss_mb();
    let t = Instant::now();
    let mut engine = Engine::load(
        &model,
        std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 4)),
    )
    .unwrap();
    let load = t.elapsed();
    let t = Instant::now();
    engine.synth(&g2p.phonemize("Ready."), SPEED).unwrap();
    let warm = t.elapsed();
    if show {
        println!("phonemes: {}", g2p.phonemize(text));
    }
    let (audio, first, total) = render(&mut engine, &g2p, text).unwrap();
    let secs = audio.len() as f32 / SAMPLE_RATE as f32;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(out.as_str(), spec).unwrap();
    for s in &audio {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
    println!(
        "files {:.2}s, lexicon {} words in {:.0} ms (+{:.0} MB), model load {:.0} ms, warm-up {:.0} ms, espeak-ng {}",
        files.as_secs_f32(),
        g2p.lex.len(),
        lex_t.as_secs_f32() * 1000.0,
        lex_rss - base,
        load.as_secs_f32() * 1000.0,
        warm.as_secs_f32() * 1000.0,
        if g2p.fallback.has_espeak() { "found" } else { "not installed" }
    );
    println!(
        "first audio {:.0} ms, total {:.2}s for {:.2}s of speech (RTF {:.3}), resident {:.0} MB (+{:.0} MB over the bare process)",
        first * 1000.0,
        total,
        secs,
        total / secs,
        rss_mb(),
        rss_mb() - base
    );
    // the app's path: the voice thread streaming chunks, timed from `speak`
    let (tx, rx) = std::sync::mpsc::channel();
    let speaker = Speaker::start(data_dir(), move |c| {
        let _ = tx.send((Instant::now(), c.first, c.samples.len()));
    });
    while !speaker.ready() && !speaker.failed() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut firsts = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        speaker.speak(text).unwrap();
        let (at, is_first, _) = rx.recv().unwrap();
        assert!(is_first);
        firsts.push((at - t).as_secs_f32() * 1000.0);
        // let the rest finish before the next run
        while rx
            .recv_timeout(std::time::Duration::from_millis(1500))
            .is_ok()
        {}
    }
    firsts.sort_by(f32::total_cmp);
    println!(
        "streaming: speak() to first chunk {:.0} ms median ({:.0}-{:.0} ms over 5 runs)",
        firsts[2], firsts[0], firsts[4]
    );
}
