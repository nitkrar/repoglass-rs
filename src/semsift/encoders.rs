//! Text to vectors. Only `encode` and `encode_query` apply the model's
//! document and query prefixes, so no backend can skip them.

use super::codec::pairwise;
use super::hub;
use super::space::{resolve_prefix, Side, VectorSpace};
use half::f16;
use tokenizers::Tokenizer;

pub trait Backend: Send + Sync {
    fn dims(&self) -> usize;
    fn encode_raw(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f64>>>;
}

pub struct Encoder {
    pub name: String,
    pub backend_name: String,
    pub variant: String,
    pub query_prefix: String,
    pub doc_prefix: String,
    backend: Box<dyn Backend>,
}

impl Encoder {
    pub fn dims(&self) -> usize {
        self.backend.dims()
    }

    pub fn space(&self) -> VectorSpace {
        VectorSpace {
            model: self.name.clone(), backend: self.backend_name.clone(), variant: self.variant.clone(),
            dims: self.dims() as i64, doc_prefix: self.doc_prefix.clone(), pooling: String::new(),
        }
    }

    /// Documents, with the model's document-side marker if it has one.
    pub fn encode(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f64>>> {
        self.backend.encode_raw(&texts.iter().map(|t| format!("{}{t}", self.doc_prefix)).collect::<Vec<_>>())
    }

    pub fn encode_query(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f64>>> {
        self.backend.encode_raw(&texts.iter().map(|t| format!("{}{t}", self.query_prefix)).collect::<Vec<_>>())
    }

    pub fn static_model(model: &str, query_prefix: Option<&str>, doc_prefix: Option<&str>,
                        extra: &[(&str, &str)]) -> anyhow::Result<Encoder> {
        Ok(Encoder {
            name: model.into(),
            backend_name: "static".into(),
            variant: String::new(),
            query_prefix: resolve_prefix(query_prefix, model, Side::Query, extra),
            doc_prefix: resolve_prefix(doc_prefix, model, Side::Doc, &[]),
            backend: Box::new(StaticModel::load(model)?),
        })
    }

    pub fn http(endpoint: &str, model: &str, api_key: &str, query_prefix: Option<&str>,
                doc_prefix: Option<&str>, extra: &[(&str, &str)]) -> anyhow::Result<Encoder> {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        let mut backend = Http { endpoint: endpoint.clone(), model: model.into(), api_key: api_key.into(), dims: 0 };
        // Learned eagerly: `dims` is part of the vector space.
        backend.dims = backend.encode_raw(&["probe".into()])?[0].len();
        Ok(Encoder {
            name: model.into(),
            backend_name: "http".into(),
            variant: endpoint,
            query_prefix: resolve_prefix(query_prefix, model, Side::Query, extra),
            doc_prefix: resolve_prefix(doc_prefix, model, Side::Doc, &[]),
            backend: Box::new(backend),
        })
    }
}

const MAX_LENGTH: usize = 512;
const BATCH: usize = 1024;

/// model2vec's `StaticModel.encode`, reproducing numpy's float16
/// arithmetic: a float32 mean cast to float16, then a float16 norm whose
/// sum numpy takes pairwise in float32.
pub struct StaticModel {
    tokenizer: Tokenizer,
    embedding: Vec<f16>,
    dims: usize,
    unk: Option<u32>,
    median_token_length: usize,
    normalize: bool,
}

impl StaticModel {
    pub fn load(model: &str) -> anyhow::Result<StaticModel> {
        let dir = hub::resolve(model, &["config.json", "model.safetensors", "tokenizer.json"])?;
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| anyhow::anyhow!("{e}"))?;
        let config: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("config.json"))?)?;
        let normalize = config["normalize"].as_bool().unwrap_or(false);
        let bytes = std::fs::read(dir.join("model.safetensors"))?;
        let tensors = safetensors::SafeTensors::deserialize(&bytes)?;
        let names: Vec<String> = tensors.names().into_iter().map(String::from).collect();
        anyhow::ensure!(names == ["embeddings"],
                        "{model}: only an `embeddings` tensor is supported; found {names:?}");
        let t = tensors.tensor("embeddings")?;
        anyhow::ensure!(t.dtype() == safetensors::Dtype::F16, "{model}: embeddings must be float16");
        let dims = t.shape()[1];
        let embedding: Vec<f16> = t.data().chunks_exact(2).map(|b| f16::from_le_bytes([b[0], b[1]])).collect();
        let vocab = tokenizer.get_vocab(true);
        let unk = vocab.get("[UNK]").copied();
        let mut lens: Vec<usize> = vocab.keys().map(|k| k.chars().count()).collect();
        lens.sort_unstable();
        // int(np.median(...)): the mean of the middle two for an even count.
        let n = lens.len();
        let median = if n % 2 == 1 { lens[n / 2] as f64 } else { (lens[n / 2 - 1] + lens[n / 2]) as f64 / 2.0 };
        Ok(StaticModel { tokenizer, embedding, dims, unk, median_token_length: median as usize, normalize })
    }

    /// One batch's token ids, `[UNK]` removed and truncated to 512. No
    /// padding, so a text's ids do not depend on its batch.
    fn batch_ids(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<u32>>> {
        let mut ids: Vec<Vec<u32>> = Vec::with_capacity(texts.len());
        for t in texts {
            let cut = crate::pyfmt::char_prefix(t, MAX_LENGTH * self.median_token_length);
            let enc = self.tokenizer.encode_fast(cut, false).map_err(|e| anyhow::anyhow!("{e}"))?;
            ids.push(enc.get_ids().to_vec());
        }
        for row in ids.iter_mut() {
            if let Some(unk) = self.unk {
                row.retain(|&i| i != unk);
            }
            row.truncate(MAX_LENGTH);
        }
        Ok(ids)
    }

    /// The float16 mean numpy's `emb.mean(axis=0)` returns.
    fn mean(&self, ids: &[u32]) -> Vec<f16> {
        let d = self.dims;
        let mut acc = vec![0f32; d];
        for &id in ids {
            let row = &self.embedding[id as usize * d..(id as usize + 1) * d];
            for (a, v) in acc.iter_mut().zip(row) {
                *a += v.to_f32();
            }
        }
        let n = ids.len() as f32;
        acc.iter().map(|a| f16::from_f32(a / n)).collect()
    }

    fn encode_batch(&self, batch: &[String]) -> anyhow::Result<Vec<Vec<f64>>> {
        let ids = self.batch_ids(batch)?;
        let means: Vec<Option<Vec<f16>>> = ids.iter().map(|i| (!i.is_empty()).then(|| self.mean(i))).collect();
        // One float64 zeros row makes numpy's stacked batch float64.
        if means.iter().all(Option::is_some) {
            return Ok(means.into_iter().map(|m| {
                let m = m.unwrap();
                if self.normalize { normalise_f16(&m) } else { m.iter().map(|v| v.to_f64()).collect() }
            }).collect());
        }
        Ok(means.into_iter().map(|m| {
            let row: Vec<f64> = match m {
                Some(m) => m.iter().map(|v| v.to_f64()).collect(),
                None => vec![0.0; self.dims],
            };
            if self.normalize { normalise_f64(&row) } else { row }
        }).collect())
    }
}

impl Backend for StaticModel {
    fn dims(&self) -> usize {
        self.dims
    }

    /// Batches are independent, so they run on every core; the output is
    /// the same as encoding them in turn.
    fn encode_raw(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f64>>> {
        let batches: Vec<&[String]> = texts.chunks(BATCH).collect();
        if batches.len() <= 1 {
            let mut out = Vec::new();
            for b in batches {
                out.extend(self.encode_batch(b)?);
            }
            return Ok(out);
        }
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut out: Vec<Vec<Vec<f64>>> = vec![Vec::new(); batches.len()];
        std::thread::scope(|scope| -> anyhow::Result<()> {
            let handles: Vec<_> = (0..threads.min(batches.len())).map(|t| {
                let batches = &batches;
                scope.spawn(move || -> anyhow::Result<Vec<(usize, Vec<Vec<f64>>)>> {
                    (t..batches.len()).step_by(threads)
                        .map(|i| Ok((i, self.encode_batch(batches[i])?))).collect()
                })
            }).collect();
            for h in handles {
                for (i, rows) in h.join().expect("encoder thread")? {
                    out[i] = rows;
                }
            }
            Ok(())
        })?;
        Ok(out.into_iter().flatten().collect())
    }
}

fn normalise_f16(m: &[f16]) -> Vec<f64> {
    let squares: Vec<f32> = m.iter().map(|v| f16::from_f32(v.to_f32() * v.to_f32()).to_f32()).collect();
    let sum = f16::from_f32(0.0 + pairwise(&squares));
    let norm = f16::from_f32(sum.to_f32().sqrt());
    m.iter().map(|v| f16::from_f32(v.to_f32() / norm.to_f32()).to_f64()).collect()
}

fn normalise_f64(m: &[f64]) -> Vec<f64> {
    let squares: Vec<f64> = m.iter().map(|v| v * v).collect();
    let norm = (0.0 + pairwise(&squares)).sqrt() + 1e-32;
    m.iter().map(|v| v / norm).collect()
}

/// An OpenAI-compatible `/v1/embeddings` server.
struct Http {
    endpoint: String,
    model: String,
    api_key: String,
    dims: usize,
}

const HTTP_BATCH: usize = 64;

impl Backend for Http {
    fn dims(&self) -> usize {
        self.dims
    }

    fn encode_raw(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f64>>> {
        let mut out = Vec::new();
        for batch in texts.chunks(HTTP_BATCH) {
            out.extend(self.post(batch)?);
        }
        Ok(out)
    }
}

impl Http {
    fn post(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f64>>> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(300))).build().into();
        let mut req = agent.post(&format!("{}/v1/embeddings", self.endpoint))
            .header("Content-Type", "application/json");
        if !self.api_key.is_empty() {
            req = req.header("Authorization", &format!("Bearer {}", self.api_key));
        }
        let payload: serde_json::Value = req
            .send_json(serde_json::json!({"input": texts, "model": self.model}))?
            .body_mut().read_json()?;
        let items = payload["data"].as_array().ok_or_else(|| anyhow::anyhow!("embedding response has no data"))?;
        let mut indexed: Vec<(i64, Vec<f64>)> = Vec::new();
        for item in items {
            let index = item["index"].as_i64().ok_or_else(|| anyhow::anyhow!("embedding response index is not an int"))?;
            let vector: Vec<f64> = item["embedding"].as_array().ok_or_else(|| anyhow::anyhow!("no embedding"))?
                .iter().map(|v| v.as_f64().filter(|x| x.is_finite())
                    .ok_or_else(|| anyhow::anyhow!("embedding response contains a non-finite value")))
                .collect::<anyhow::Result<_>>()?;
            indexed.push((index, vector));
        }
        let mut indices: Vec<i64> = indexed.iter().map(|p| p.0).collect();
        indices.sort_unstable();
        anyhow::ensure!(indices == (0..texts.len() as i64).collect::<Vec<_>>(),
                        "embedding response indices {indices:?}; expected 0..{}", texts.len());
        indexed.sort_by_key(|p| p.0);
        let widths: std::collections::BTreeSet<usize> = indexed.iter().map(|p| p.1.len()).collect();
        anyhow::ensure!(widths.len() <= 1, "embedding response has mixed vector widths {widths:?}");
        anyhow::ensure!(widths.iter().all(|w| *w > 0), "embedding response contains empty vectors");
        if self.dims > 0 {
            anyhow::ensure!(widths.iter().all(|w| *w == self.dims),
                            "embedding width {widths:?}; expected {}", self.dims);
        }
        Ok(indexed.into_iter().map(|p| p.1).collect())
    }
}
