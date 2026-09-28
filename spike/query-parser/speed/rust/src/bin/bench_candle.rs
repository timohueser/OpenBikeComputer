//! cargo run --release --features candle --bin bench_candle -- ../work/cut-50k ../work/onnx/cut-50k/expected_ids.json [runs]
//! Loads the cut encoder (config.json, model.safetensors, tokenizer.json) with candle's ModernBERT,
//! adds random heads, and times tokenize + forward. RAYON_NUM_THREADS=1 for one thread.
use candle_core::{DType, Device, Module, Tensor};
use candle_nn::{Linear, VarBuilder};
use candle_transformers::models::modernbert::{Config, ModernBert};
use std::path::Path;
use std::time::Instant;
use tokenizers::Tokenizer;

fn pct(xs: &mut [f64], p: f64) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[((p / 100.0) * (xs.len() - 1) as f64).round() as usize]
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output();
    out.ok().and_then(|o| String::from_utf8(o.stdout).ok()?.trim().parse::<f64>().ok()).unwrap_or(0.0) / 1024.0
}

/// candle reads the transformers 4 config keys; transformers 5 writes rope_parameters instead.
fn config(dir: &Path) -> Config {
    let c: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    let u = |k: &str| c[k].as_u64().unwrap() as usize;
    let rope =
        |kind: &str, legacy: &str| c["rope_parameters"][kind]["rope_theta"].as_f64().or(c[legacy].as_f64()).unwrap();
    Config {
        vocab_size: u("vocab_size"),
        hidden_size: u("hidden_size"),
        num_hidden_layers: u("num_hidden_layers"),
        num_attention_heads: u("num_attention_heads"),
        intermediate_size: u("intermediate_size"),
        max_position_embeddings: u("max_position_embeddings"),
        layer_norm_eps: c["norm_eps"].as_f64().unwrap(),
        pad_token_id: u("pad_token_id") as u32,
        global_attn_every_n_layers: u("global_attn_every_n_layers"),
        global_rope_theta: rope("full_attention", "global_rope_theta"),
        local_attention: u("local_attention"),
        local_rope_theta: rope("sliding_attention", "local_rope_theta"),
        classifier_config: None,
    }
}

fn main() -> candle_core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(&args[1]);
    let runs: usize = args.get(3).map(|r| r.parse().unwrap()).unwrap_or(300);
    let dev = Device::Cpu;
    let sentences = parser_bench::expected_ids(&std::fs::read_to_string(&args[2]).unwrap());

    let rss0 = rss_mb();
    let t = Instant::now();
    let cfg = config(dir);
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], DType::F32, &dev)? };
    let vb = vb.rename_f(|n: &str| n.strip_prefix("model.").unwrap_or(n).to_string());
    let encoder = ModernBert::load(vb, &cfg)?;
    let intent = Linear::new(
        Tensor::randn(0f32, 0.05, (12, cfg.hidden_size), &dev)?,
        Some(Tensor::zeros(12, DType::F32, &dev)?),
    );
    let tags = Linear::new(
        Tensor::randn(0f32, 0.05, (45, cfg.hidden_size), &dev)?,
        Some(Tensor::zeros(45, DType::F32, &dev)?),
    );
    let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).unwrap();
    let load_ms = t.elapsed().as_secs_f64() * 1e3;

    let parse = |s: &str| -> candle_core::Result<(f64, f64)> {
        let t = Instant::now();
        let ids = tokenizer.encode(s, true).unwrap().get_ids().to_vec();
        let tok = t.elapsed().as_secs_f64() * 1e3;
        let input = Tensor::new(ids.as_slice(), &dev)?.unsqueeze(0)?;
        let mask = Tensor::ones((1, ids.len()), DType::U32, &dev)?;
        let h = encoder.forward(&input, &mask)?;
        let _intent = intent.forward(&h.get_on_dim(1, 0)?)?.to_vec2::<f32>()?;
        let _tags = tags.forward(&h)?.to_vec3::<f32>()?;
        Ok((t.elapsed().as_secs_f64() * 1e3, tok))
    };
    let (first, _) = parse(&sentences[0].0)?;
    let mut times = Vec::new();
    let mut toks = Vec::new();
    for i in 0..runs {
        let (a, b) = parse(&sentences[(i + 1) % sentences.len()].0)?;
        times.push(a);
        toks.push(b);
    }
    println!(
        "candle {}: load {load_ms:.0} ms, first {first:.1} ms, median {:.1} ms, p95 {:.1} ms, tokenize {:.3} ms, rss {:.0} MB (+{:.0})",
        dir.display(),
        pct(&mut times, 50.0),
        pct(&mut times, 95.0),
        pct(&mut toks, 50.0),
        rss_mb(),
        rss_mb() - rss0
    );
    Ok(())
}
