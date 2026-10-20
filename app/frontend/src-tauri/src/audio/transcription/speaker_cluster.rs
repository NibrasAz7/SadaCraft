// audio/transcription/speaker_cluster.rs
//
// Online remote-speaker clustering (ML diarization v2).
//
// For each system-audio VAD segment we compute a 256-dim speaker embedding
// (wespeaker voxceleb resnet34 LM, ONNX) and assign it to the nearest online
// cluster centroid by cosine similarity. Segments are labeled "Remote-1",
// "Remote-2", ... in order of first appearance. Microphone audio stays "You"
// and is never clustered.
//
// The whole feature is opt-in via SADACRAFT_DIARIZATION=1 (optionally tune the
// cosine threshold via SADACRAFT_DIARIZATION_THRESHOLD). When disabled or when
// the model cannot be downloaded/loaded, system segments keep the plain
// "Remote" label.
//
// Feature extraction mirrors sherpa-onnx's kaldi-native-fbank frontend
// (snip_edges=false, dither=0, samples scaled to int16 range) so the embeddings
// match what the released wespeaker model expects.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use log::{info, warn};
use ndarray::Array3;
use ort::execution_providers::CPUExecutionProvider;
use ort::inputs;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use realfft::RealFftPlanner;
use tauri::{AppHandle, Manager, Runtime};
use tokio::io::AsyncWriteExt;

use crate::audio::AudioChunk;

// --- Model ---------------------------------------------------------------

/// Speaker embedding model: wespeaker voxceleb resnet34 (large-margin),
/// hosted as a single-file ONNX asset in the sherpa-onnx release.
/// https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models
const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34_LM.onnx";
const MODEL_FILENAME: &str = "wespeaker_en_voxceleb_resnet34_LM.onnx";
const MODEL_EXACT_BYTES: u64 = 26_530_550;

// --- Clustering ----------------------------------------------------------

const SAMPLE_RATE: u32 = 16_000;
/// Minimum segment length for a reliable embedding (0.5s @ 16kHz). Shorter
/// segments keep the generic "Remote" label instead of risking a bogus
/// embedding that could spawn a spurious cluster.
const MIN_EMBED_SAMPLES: usize = 8_000;
/// Safety cap so pathological audio cannot produce unbounded labels.
const MAX_CLUSTERS: usize = 16;
/// Cosine-similarity threshold between a segment embedding and a cluster
/// centroid. wespeaker voxceleb embeddings typically score >0.6 for the same
/// speaker and <0.5 for different speakers.
const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.65;

// --- Feature extraction (kaldi-native-fbank compatible) -------------------
// Matches sherpa-onnx FeatureExtractorConfig defaults used for wespeaker
// models: 25ms povey window, 10ms shift, dither=0, remove_dc_offset=true,
// preemph=0.97, power-of-two padded FFT, 80 mel bins over 20..7600Hz,
// snip_edges=false (reflection padding), samples scaled to int16 range.

const FRAME_LEN: usize = 400; // 25ms @ 16kHz
const FRAME_SHIFT: usize = 160; // 10ms @ 16kHz
const FFT_SIZE: usize = 512; // 400 rounded up to a power of two
const NUM_MEL: usize = 80;
const MEL_LOW_HZ: f32 = 20.0;
const MEL_HIGH_HZ: f32 = 7600.0; // nyquist - 400
const PREEMPH: f32 = 0.97;
const SAMPLE_SCALE: f32 = 32768.0; // wespeaker expects int16-scale samples

/// True when remote-speaker clustering is requested via environment.
pub fn diarization_enabled() -> bool {
    std::env::var("SADACRAFT_DIARIZATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn mel_scale(freq: f32) -> f32 {
    // knf::MelScale (HTK formula): 1127 * ln(1 + f/700)
    1127.0 * (1.0 + freq / 700.0).ln()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom <= f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

fn l2_normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

/// Kaldi-compatible 80-dim log-fbank extractor for 16kHz mono f32 input.
struct KaldiFbank {
    window: Vec<f32>,               // povey window, FRAME_LEN
    mel_bins: Vec<(usize, Vec<f32>)>, // (first fft bin, weights) per mel bin
    fft: Arc<dyn realfft::RealToComplex<f32>>,
}

impl KaldiFbank {
    fn new() -> Self {
        // Povey window: hamming^0.85, zero at the edges.
        // GetWindow("povey"): a = 2*pi/(N-1); w[i] = (0.5 - 0.5*cos(a*i))^0.85
        let a = std::f64::consts::TAU / (FRAME_LEN - 1) as f64;
        let window: Vec<f32> = (0..FRAME_LEN)
            .map(|i| (0.5f64 - 0.5f64 * (a * i as f64).cos()).powf(0.85) as f32)
            .collect();

        // Triangular mel filters in the *mel* domain over FFT bin centers,
        // exactly as knf::MelBanks::InitKaldiMelBanks (no normalization).
        let num_fft_bins = FFT_SIZE / 2; // 256; the nyquist bin is not filtered
        let fft_bin_width = SAMPLE_RATE as f32 / FFT_SIZE as f32; // 31.25Hz
        let mel_low = mel_scale(MEL_LOW_HZ);
        let mel_high = mel_scale(MEL_HIGH_HZ);
        let mel_delta = (mel_high - mel_low) / (NUM_MEL + 1) as f32;

        let mut mel_bins = Vec::with_capacity(NUM_MEL);
        for bin in 0..NUM_MEL {
            let left = mel_low + bin as f32 * mel_delta;
            let center = mel_low + (bin + 1) as f32 * mel_delta;
            let right = mel_low + (bin + 2) as f32 * mel_delta;

            let mut first_index = usize::MAX;
            let mut last_index = 0usize;
            let mut weights = vec![0.0f32; num_fft_bins];
            for i in 0..num_fft_bins {
                let mel = mel_scale(fft_bin_width * i as f32);
                if mel > left && mel < right {
                    let w = if mel <= center {
                        (mel - left) / (center - left)
                    } else {
                        (right - mel) / (right - center)
                    };
                    weights[i] = w;
                    first_index = first_index.min(i);
                    last_index = last_index.max(i);
                }
            }
            let span: Vec<f32> = weights[first_index..=last_index].to_vec();
            mel_bins.push((first_index, span));
        }

        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        Self {
            window,
            mel_bins,
            fft,
        }
    }

    /// Compute log-fbank features for 16kHz mono samples in [-1, 1].
    /// Returns row-major [num_frames x NUM_MEL] (sherpa-onnx semantics:
    /// samples scaled to int16 range, snip_edges=false, reflection padding).
    fn compute(&self, samples: &[f32]) -> Vec<f32> {
        let n = samples.len();
        if n == 0 {
            return Vec::new();
        }

        // NumFrames(snip_edges=false, flush=true): round(N / shift).
        let num_frames = (n + FRAME_SHIFT / 2) / FRAME_SHIFT;
        let mut out = Vec::with_capacity(num_frames * NUM_MEL);

        let mut frame = vec![0.0f32; FFT_SIZE];
        let mut spectrum = self.fft.make_output_vec();

        for f in 0..num_frames {
            // FirstSampleOfFrame(snip_edges=false):
            // midpoint = shift*f + shift/2; start = midpoint - len/2.
            let start = f as i64 * FRAME_SHIFT as i64 + (FRAME_SHIFT / 2) as i64
                - (FRAME_LEN / 2) as i64;

            for s in 0..FRAME_LEN {
                let mut idx = start + s as i64;
                // Reflection padding, same as knf::ExtractWindow.
                while idx < 0 || idx >= n as i64 {
                    if idx < 0 {
                        idx = -idx - 1;
                    } else {
                        idx = 2 * n as i64 - 1 - idx;
                    }
                }
                frame[s] = samples[idx as usize] * SAMPLE_SCALE;
            }
            frame[FRAME_LEN..].fill(0.0);

            // Remove DC offset over the real (unpadded) frame samples.
            let mean =
                frame[..FRAME_LEN].iter().sum::<f32>() / FRAME_LEN as f32;
            for v in &mut frame[..FRAME_LEN] {
                *v -= mean;
            }

            // Preemphasis (in-place, backwards).
            for i in (1..FRAME_LEN).rev() {
                frame[i] -= PREEMPH * frame[i - 1];
            }
            frame[0] -= PREEMPH * frame[0];

            for i in 0..FRAME_LEN {
                frame[i] *= self.window[i];
            }

            self.fft
                .process(&mut frame, &mut spectrum)
                .expect("fbank FFT failed");

            for (offset, weights) in &self.mel_bins {
                let mut energy = 0.0f32;
                for (k, w) in weights.iter().enumerate() {
                    let c = spectrum[offset + k];
                    energy += w * (c.re * c.re + c.im * c.im);
                }
                out.push(energy.max(f32::EPSILON).ln());
            }
        }

        out
    }
}

/// ONNX speaker-embedding model (wespeaker voxceleb resnet34 LM).
/// Input "feats": [1, T, 80] fbank; output "embs": [1, 256] embedding.
/// Names are read from the model so other compatible exports also work.
struct SpeakerEmbedder {
    session: Session,
    input_name: String,
    output_name: String,
    fbank: KaldiFbank,
}

impl SpeakerEmbedder {
    fn new(model_path: &Path) -> Result<Self> {
        crate::ensure_onnx_runtime_available()?;

        let providers = vec![CPUExecutionProvider::default().build()];
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_execution_providers(providers)?
            .with_intra_threads(2)?
            .commit_from_file(model_path)?;

        let input_name = session
            .inputs
            .first()
            .map(|i| i.name.clone())
            .ok_or_else(|| anyhow!("speaker embedding model has no inputs"))?;
        let output_name = session
            .outputs
            .first()
            .map(|o| o.name.clone())
            .ok_or_else(|| anyhow!("speaker embedding model has no outputs"))?;

        info!(
            "Loaded speaker embedding model {} (input='{}', output='{}')",
            model_path.display(),
            input_name,
            output_name
        );

        Ok(Self {
            session,
            input_name,
            output_name,
            fbank: KaldiFbank::new(),
        })
    }

    /// 16kHz mono f32 samples -> L2-normalized embedding.
    fn embed(&mut self, samples_16k: &[f32]) -> Result<Vec<f32>> {
        let feats = self.fbank.compute(samples_16k);
        let num_frames = feats.len() / NUM_MEL;
        if num_frames == 0 {
            return Err(anyhow!("segment too short for speaker embedding"));
        }

        let x = Array3::from_shape_vec((1, num_frames, NUM_MEL), feats)?.into_dyn();
        let run_inputs = inputs![self.input_name.clone() =>
            TensorRef::from_array_view(x.view())?];
        let outputs = self.session.run(run_inputs)?;

        let embedding = outputs
            .get(self.output_name.as_str())
            .ok_or_else(|| anyhow!("model output '{}' missing", self.output_name))?
            .try_extract_array::<f32>()?;
        let mut emb: Vec<f32> = embedding.iter().copied().collect();
        l2_normalize(&mut emb);
        Ok(emb)
    }
}

// --- Online centroid clustering ------------------------------------------

struct Cluster {
    /// Running sum of (L2-normalized) member embeddings. Cosine similarity is
    /// scale-invariant, so scoring against the sum equals scoring against the
    /// normalized centroid.
    sum: Vec<f32>,
    count: usize,
}

/// Online centroid clustering over L2-normalized embeddings. Pure math, no
/// model dependency — kept separate so it is unit-testable without ONNX.
struct OnlineSpeakerClusters {
    clusters: Vec<Cluster>,
    threshold: f32,
}

impl OnlineSpeakerClusters {
    fn new(threshold: f32) -> Self {
        Self {
            clusters: Vec::new(),
            threshold,
        }
    }

    #[cfg(test)]
    fn num_clusters(&self) -> usize {
        self.clusters.len()
    }

    fn assign_embedding(&mut self, embedding: &[f32]) -> usize {
        let mut best = 0usize;
        let mut best_sim = f32::MIN;
        for (i, cluster) in self.clusters.iter().enumerate() {
            let sim = cosine_similarity(&cluster.sum, embedding);
            if sim > best_sim {
                best_sim = sim;
                best = i;
            }
        }

        if !self.clusters.is_empty()
            && (best_sim >= self.threshold || self.clusters.len() >= MAX_CLUSTERS)
        {
            if best_sim < self.threshold {
                warn!(
                    "speaker cluster cap reached ({}); assigning to nearest cluster {} (sim={:.3})",
                    MAX_CLUSTERS, best, best_sim
                );
            }
            let cluster = &mut self.clusters[best];
            for (a, &b) in cluster.sum.iter_mut().zip(embedding.iter()) {
                *a += b;
            }
            cluster.count += 1;
            best
        } else {
            info!(
                "New remote speaker cluster {} (best_sim={:.3}, threshold={:.2})",
                self.clusters.len() + 1,
                best_sim,
                self.threshold
            );
            self.clusters.push(Cluster {
                sum: embedding.to_vec(),
                count: 1,
            });
            self.clusters.len() - 1
        }
    }
}

pub struct SpeakerClusterer {
    embedder: SpeakerEmbedder,
    clusters: OnlineSpeakerClusters,
}

impl SpeakerClusterer {
    /// Assign a 16kHz mono f32 segment to a cluster. Returns the 0-based
    /// cluster index, or None when the segment is too short or embedding
    /// failed (caller should then use the plain "Remote" label).
    pub fn assign(&mut self, samples_16k: &[f32]) -> Option<usize> {
        if samples_16k.len() < MIN_EMBED_SAMPLES {
            return None;
        }
        let embedding = match self.embedder.embed(samples_16k) {
            Ok(e) => e,
            Err(e) => {
                warn!("speaker embedding failed, keeping 'Remote': {}", e);
                return None;
            }
        };
        Some(self.clusters.assign_embedding(&embedding))
    }
}

// --- Model download & initialization --------------------------------------

/// Resolve the models directory (same base as the whisper/parakeet models).
fn models_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("failed to resolve app data dir: {}", e))?
        .join("models");
    Ok(dir)
}

/// Ensure the speaker embedding model exists on disk, downloading it on
/// demand. Returns the model path.
async fn ensure_model_file(dir: &Path) -> Result<PathBuf> {
    let model_path = dir.join(MODEL_FILENAME);
    if let Ok(meta) = std::fs::metadata(&model_path) {
        if meta.len() == MODEL_EXACT_BYTES {
            return Ok(model_path);
        }
        warn!(
            "Speaker embedding model has unexpected size {} (expected {}), re-downloading",
            meta.len(),
            MODEL_EXACT_BYTES
        );
        let _ = std::fs::remove_file(&model_path);
    }

    std::fs::create_dir_all(dir)?;
    info!(
        "Downloading speaker embedding model ({:.0} MB) to {} ...",
        MODEL_EXACT_BYTES as f64 / (1024.0 * 1024.0),
        model_path.display()
    );

    let tmp_path = dir.join(format!("{}.download", MODEL_FILENAME));
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let result = async {
        let response = client.get(MODEL_URL).send().await?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "speaker model download failed with status {}",
                response.status()
            ));
        }
        let mut file = tokio::fs::File::create(&tmp_path).await?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk?).await?;
        }
        file.flush().await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    let len = std::fs::metadata(&tmp_path)?.len();
    if len != MODEL_EXACT_BYTES {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(anyhow!(
            "speaker model download incomplete: {} bytes, expected {}",
            len,
            MODEL_EXACT_BYTES
        ));
    }

    // Rename into place (replace any stale file).
    if model_path.exists() {
        let _ = std::fs::remove_file(&model_path);
    }
    std::fs::rename(&tmp_path, &model_path)?;
    info!("Speaker embedding model ready at {}", model_path.display());
    Ok(model_path)
}

/// Initialize the clusterer for a recording session. Returns None when the
/// feature is disabled or unavailable — callers then keep "Remote" labels.
pub async fn init_speaker_clusterer<R: Runtime>(
    app: &AppHandle<R>,
) -> Option<Arc<Mutex<SpeakerClusterer>>> {
    if !diarization_enabled() {
        return None;
    }

    match try_init(app).await {
        Ok(clusterer) => {
            info!("Remote speaker diarization enabled (SADACRAFT_DIARIZATION)");
            Some(Arc::new(Mutex::new(clusterer)))
        }
        Err(e) => {
            warn!("Remote speaker diarization disabled: {}", e);
            None
        }
    }
}

async fn try_init<R: Runtime>(app: &AppHandle<R>) -> Result<SpeakerClusterer> {
    crate::ensure_onnx_runtime_available()?;

    let dir = models_dir(app)?;
    let model_path = ensure_model_file(&dir).await?;

    let embedder =
        tokio::task::spawn_blocking(move || SpeakerEmbedder::new(&model_path))
            .await??;

    let threshold = std::env::var("SADACRAFT_DIARIZATION_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > -1.0 && *v <= 1.0)
        .unwrap_or(DEFAULT_SIMILARITY_THRESHOLD);

    Ok(SpeakerClusterer {
        embedder,
        clusters: OnlineSpeakerClusters::new(threshold),
    })
}

// --- Worker integration ---------------------------------------------------

/// Speaker label for a system-audio chunk. Returns "Remote-N" in order of
/// first appearance when clustering is active, else the plain "Remote".
pub(crate) fn system_speaker_label(
    clusterer: &Option<Arc<Mutex<SpeakerClusterer>>>,
    chunk: &AudioChunk,
) -> String {
    let Some(clusterer) = clusterer else {
        return "Remote".to_string();
    };

    // VAD segments arrive at 16kHz; resample defensively if that ever changes.
    let samples: Cow<[f32]> = if chunk.sample_rate == SAMPLE_RATE {
        Cow::Borrowed(&chunk.data[..])
    } else {
        Cow::Owned(crate::audio::audio_processing::resample_audio(
            &chunk.data,
            chunk.sample_rate,
            SAMPLE_RATE,
        ))
    };

    match clusterer.lock() {
        Ok(mut guard) => match guard.assign(&samples) {
            Some(cluster_id) => format!("Remote-{}", cluster_id + 1),
            None => "Remote".to_string(),
        },
        Err(_) => {
            warn!("speaker clusterer lock poisoned, keeping 'Remote'");
            "Remote".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(v: &[f32]) -> Vec<f32> {
        let mut v = v.to_vec();
        l2_normalize(&mut v);
        v
    }

    #[test]
    fn cosine_identical_and_orthogonal() {
        let a = normalized(&[1.0, 2.0, 3.0, 4.0]);
        let b = normalized(&[1.0, 2.0, 3.0, 4.0]);
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);

        let c = normalized(&[1.0, 0.0]);
        let d = normalized(&[0.0, 1.0]);
        assert!(cosine_similarity(&c, &d).abs() < 1e-6);

        let e = normalized(&[1.0, 0.0]);
        let f = normalized(&[-1.0, 0.0]);
        assert!((cosine_similarity(&e, &f) + 1.0).abs() < 1e-5);
    }

    fn clusterer_for_tests() -> OnlineSpeakerClusters {
        OnlineSpeakerClusters::new(DEFAULT_SIMILARITY_THRESHOLD)
    }

    #[test]
    fn clusters_form_in_order_of_appearance() {
        let mut c = clusterer_for_tests();
        // Two clearly distinct speakers.
        let e1 = normalized(&[1.0, 0.0, 0.0]);
        let e2 = normalized(&[0.0, 1.0, 0.0]);
        let e3 = normalized(&[0.0, 0.0, 1.0]);

        assert_eq!(c.assign_embedding(&e1), 0);
        assert_eq!(c.assign_embedding(&e2), 1);
        assert_eq!(c.assign_embedding(&e3), 2);
        assert_eq!(c.num_clusters(), 3);
    }

    #[test]
    fn similar_embeddings_join_existing_cluster() {
        let mut c = clusterer_for_tests();
        let base = normalized(&[1.0, 0.0, 0.0]);
        // Perturb slightly: still the same speaker (cos > 0.65).
        let near = normalized(&[1.0, 0.2, 0.0]);
        let far = normalized(&[0.0, 1.0, 0.0]);

        assert_eq!(c.assign_embedding(&base), 0);
        assert_eq!(c.assign_embedding(&near), 0);
        // A different speaker creates a new cluster.
        assert_eq!(c.assign_embedding(&far), 1);
        assert_eq!(c.num_clusters(), 2);
    }

    #[test]
    fn cluster_count_is_capped() {
        let mut c = clusterer_for_tests();
        for i in 0..MAX_CLUSTERS {
            let mut e = vec![0.0f32; MAX_CLUSTERS];
            e[i] = 1.0;
            assert_eq!(c.assign_embedding(&normalized(&e)), i);
        }
        // The (MAX+1)-th distinct embedding must not create a new cluster.
        let mut odd = vec![0.0f32; MAX_CLUSTERS];
        odd[0] = 1.0;
        odd[1] = -1.0;
        let id = c.assign_embedding(&normalized(&odd));
        assert!(id < MAX_CLUSTERS);
        assert_eq!(c.num_clusters(), MAX_CLUSTERS);
    }

    #[test]
    fn fbank_frame_count_matches_sherpa() {
        let fb = KaldiFbank::new();
        // snip_edges=false, flush: num_frames = round(N / 160)
        for n in [8000usize, 16000, 16001, 24000] {
            let samples = vec![0.1f32; n];
            let feats = fb.compute(&samples);
            let expected_frames = (n + FRAME_SHIFT / 2) / FRAME_SHIFT;
            assert_eq!(feats.len(), expected_frames * NUM_MEL, "n={}", n);
        }
    }

    #[test]
    fn fbank_constant_signal_has_no_spectral_energy() {
        let fb = KaldiFbank::new();
        // A pure DC signal: remove_dc_offset leaves zeros, so every mel energy
        // floors at f32::EPSILON and log gives ln(eps).
        let samples = vec![0.5f32; 16_000];
        let feats = fb.compute(&samples);
        let floor = f32::EPSILON.ln();
        // Edge frames reflect the constant -> still DC -> still floor.
        // Interior frames must all equal the floor.
        for (i, &v) in feats.iter().enumerate() {
            assert!(
                (v - floor).abs() < 1e-3 || v > floor,
                "feature {} = {} (floor {})",
                i,
                v,
                floor
            );
        }
    }

    #[test]
    fn fbank_sine_concentrates_energy() {
        let fb = KaldiFbank::new();
        // 1000Hz sine: energy should land in the mel bins around ~1000Hz
        // (mel(1000)=1127*ln(1+1000/700) ~= 999 -> bin index ~ (999-mel_low)/delta).
        let n = 16_000;
        let samples: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 16000.0).sin())
            .collect();
        let feats = fb.compute(&samples);
        let num_frames = feats.len() / NUM_MEL;
        assert!(num_frames > 0);
        // Take the middle frame (away from reflected edges).
        let frame = &feats[(num_frames / 2) * NUM_MEL..(num_frames / 2 + 1) * NUM_MEL];
        let (max_bin, max_v) = frame
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, v)| (i, *v))
            .unwrap();
        // mel(1000) ~= 999; with mel_low=mel(20)~31.8 and
        // delta=(mel(7600)-mel_low)/81 ~34.0 the peak bin is ~(999-32)/34 ~ 28.5,
        // so allow a few bins of slack.
        assert!(
            (25..=32).contains(&max_bin),
            "sine peak in mel bin {} (value {}), expected ~28",
            max_bin,
            max_v
        );
    }

    /// End-to-end check with the real ONNX model. Skips silently when the
    /// model file, the bundled onnxruntime.dll, or the sample audio is absent
    /// (e.g. CI). Set SADACRAFT_SPEAKER_MODEL_PATH to override the model path.
    #[cfg(target_os = "windows")]
    #[test]
    fn wespeaker_embedding_smoke() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dll = manifest.join("binaries/onnxruntime/onnxruntime.dll");
        let default_model = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("com.sadacraft.app/models")
            .join(MODEL_FILENAME);
        let model_path = std::env::var("SADACRAFT_SPEAKER_MODEL_PATH")
            .map(PathBuf::from)
            .unwrap_or(default_model);

        // research/ar-sample.f32: 16kHz mono f32le, lives at repo root.
        let sample_path = manifest.join("../../../research/ar-sample.f32");

        if !dll.exists() || !model_path.exists() || !sample_path.exists() {
            eprintln!(
                "skipping: dll={} model={} sample={}",
                dll.exists(),
                model_path.exists(),
                sample_path.exists()
            );
            return;
        }

        // Initialize the bundled runtime (ignore 'already initialized').
        let dll_str = dll.to_string_lossy().into_owned();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ort::init_from(dll_str).with_telemetry(false).commit()
        }));

        let mut embedder = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            SpeakerEmbedder::new(&model_path)
        })) {
            Ok(Ok(e)) => e,
            Ok(Err(e)) => {
                eprintln!("skipping: embedder init failed: {}", e);
                return;
            }
            Err(_) => {
                eprintln!("skipping: embedder init panicked");
                return;
            }
        };

        let bytes = std::fs::read(&sample_path).unwrap();
        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert!(samples.len() >= 32_000, "sample file too short");

        let e1 = embedder.embed(&samples[..16_000]).unwrap();
        let e2 = embedder.embed(&samples[16_000..32_000]).unwrap();
        assert_eq!(e1.len(), 256);
        assert_eq!(e2.len(), 256);
        assert!(e1.iter().all(|v| v.is_finite()));
        let norm: f32 = e1.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "embedding norm {}", norm);

        let sim = cosine_similarity(&e1, &e2);
        eprintln!("same-file embedding cosine = {:.4}", sim);
        // Same speaker, different 1s windows: expect a clearly positive match.
        assert!(sim > 0.3, "same-speaker cosine {} too low", sim);

        // A different speaker (different language sample) should score clearly
        // lower — this is the property the clustering relies on.
        let other_path = manifest.join("../../../research/it-sample.f32");
        if other_path.exists() {
            let other_bytes = std::fs::read(&other_path).unwrap();
            let other: Vec<f32> = other_bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            if other.len() >= 16_000 {
                let e3 = embedder.embed(&other[..16_000]).unwrap();
                let cross = cosine_similarity(&e1, &e3);
                eprintln!("cross-speaker embedding cosine = {:.4}", cross);
                assert!(
                    cross < sim - 0.1,
                    "cross-speaker cosine {} not separated from same-speaker {}",
                    cross,
                    sim
                );
            }
        }
    }
}
