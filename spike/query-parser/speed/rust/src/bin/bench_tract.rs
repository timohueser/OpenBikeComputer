//! cargo run --release --bin bench_tract -- ../work/onnx/cut-50k/model.int8.onnx [runs]
//! Reads tokenizer.json and expected_ids.json next to the model.
use parser_bench::{expected_ids, TractParser};
use std::path::Path;
use std::time::Instant;

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output();
    out.ok().and_then(|o| String::from_utf8(o.stdout).ok()?.trim().parse::<f64>().ok()).unwrap_or(0.0) / 1024.0
}

fn pct(xs: &mut [f64], p: f64) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[((p / 100.0) * (xs.len() - 1) as f64).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let model = Path::new(&args[1]);
    let runs: usize = args.get(2).map(|r| r.parse().unwrap()).unwrap_or(300);
    let dir = model.parent().unwrap();
    let sentences = expected_ids(&std::fs::read_to_string(dir.join("expected_ids.json")).unwrap());

    let rss0 = rss_mb();
    let t = Instant::now();
    let onnx = std::fs::read(model).unwrap();
    let parser = TractParser::load(&onnx, &std::fs::read(dir.join("tokenizer.json")).unwrap()).unwrap();
    drop(onnx);
    let load_ms = t.elapsed().as_secs_f64() * 1e3;
    let mismatches = sentences.iter().filter(|(s, ids)| &parser.tokenize(s) != ids).count();

    let parse = |s: &str| {
        let t = Instant::now();
        let ids = parser.tokenize(s);
        let tok = t.elapsed().as_secs_f64() * 1e3;
        parser.run(&ids).unwrap();
        (t.elapsed().as_secs_f64() * 1e3, tok)
    };
    let (first, _) = parse(&sentences[0].0);
    let (mut times, mut toks): (Vec<f64>, Vec<f64>) =
        (0..runs).map(|i| parse(&sentences[(i + 1) % sentences.len()].0)).unzip();
    println!(
        "tract {}: load {load_ms:.0} ms, first {first:.1} ms, median {:.1} ms, p95 {:.1} ms, tokenize {:.3} ms, \
         rss {:.0} MB (+{:.0}), tokenizer mismatches {mismatches}/{}",
        model.file_name().unwrap().to_string_lossy(),
        pct(&mut times, 50.0),
        pct(&mut times, 95.0),
        pct(&mut toks, 50.0),
        rss_mb(),
        rss_mb() - rss0,
        sentences.len()
    );
}
