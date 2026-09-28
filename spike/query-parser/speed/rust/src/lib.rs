//! One parse = tokenize + joint tagger forward, with tract on the exported ONNX.
//! The same code builds natively (bench_tract) and for wasm32 (the `Model` binding).
use tokenizers::Tokenizer;
use tract_onnx::prelude::*;

pub struct TractParser {
    plan: Arc<TypedRunnableModel>,
    tokenizer: Tokenizer,
}

impl TractParser {
    pub fn load(onnx: &[u8], tokenizer_json: &[u8]) -> TractResult<Self> {
        // The export declares both inputs as i64 [1, seq]; tract keeps `seq` symbolic.
        let model = tract_onnx::onnx().model_for_read(&mut &onnx[..])?;
        let plan = model.into_optimized()?.into_runnable()?;
        let tokenizer = Tokenizer::from_bytes(tokenizer_json).map_err(|e| TractError::msg(e.to_string()))?;
        Ok(Self { plan, tokenizer })
    }

    pub fn tokenize(&self, text: &str) -> Vec<u32> {
        self.tokenizer.encode(text, true).map(|e| e.get_ids().to_vec()).unwrap_or_default()
    }

    /// Returns the intent logits and the number of tagged tokens.
    pub fn run(&self, ids: &[u32]) -> TractResult<(Vec<f32>, usize)> {
        let n = ids.len();
        let ids: Vec<i64> = ids.iter().map(|&i| i as i64).collect();
        let input = Tensor::from_shape(&[1, n], &ids)?;
        let mask = Tensor::from_shape(&[1, n], &vec![1i64; n])?;
        let out = self.plan.run(tvec!(input.into(), mask.into()))?;
        Ok((out[0].try_as_plain_ram()?.as_slice::<f32>()?.to_vec(), out[1].shape()[1]))
    }
}

/// Bench sentences with their reference token ids, as written by export_onnx.py.
pub fn expected_ids(json: &str) -> Vec<(String, Vec<u32>)> {
    let v: Vec<serde_json::Value> = serde_json::from_str(json).expect("expected_ids.json");
    v.iter()
        .map(|e| {
            let ids = e["ids"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect();
            (e["text"].as_str().unwrap().to_string(), ids)
        })
        .collect()
}

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub struct Model(super::TractParser);

    #[wasm_bindgen]
    impl Model {
        #[wasm_bindgen(constructor)]
        pub fn new(onnx: &[u8], tokenizer_json: &str) -> Result<Model, JsError> {
            super::TractParser::load(onnx, tokenizer_json.as_bytes())
                .map(Model)
                .map_err(|e| JsError::new(&format!("{e:?}")))
        }

        pub fn tokenize(&self, text: &str) -> Vec<u32> {
            self.0.tokenize(text)
        }

        pub fn run(&self, ids: &[u32]) -> Result<Vec<f32>, JsError> {
            self.0.run(ids).map(|(intent, _)| intent).map_err(|e| JsError::new(&format!("{e:?}")))
        }
    }
}
