use anyhow::Result;
use app_lib::whisper_engine::WhisperEngine;
use std::path::PathBuf;

fn read_f32le(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read audio file");
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let model = args.get(1).map(|s| s.as_str()).unwrap_or("small-q5_1");
    let models_dir = dirs::data_dir()
        .unwrap()
        .join("com.sadacraft.app")
        .join("models");

    let eng = WhisperEngine::new_with_models_dir(Some(models_dir))?;
    let models = eng.discover_models().await?;
    let entry = models
        .iter()
        .find(|m| m.name == model)
        .expect("model in catalog");
    println!("model {} status before: {:?}", model, entry.status);

    if matches!(entry.status, app_lib::whisper_engine::ModelStatus::Missing) {
        eng.download_model(model, Some(Box::new(|p: u8| {
            if p % 10 == 0 { println!("download {}%", p); }
        })))
        .await?;
    }
    eng.load_model(model).await?;
    println!("model {} loaded", model);

    let cases: Vec<(String, Option<&str>)> = if args.len() > 2 {
        vec![(args[2].clone(), args.get(3).map(|s| s.as_str()))]
    } else {
        vec![
            ("../../../research/ar-sample.f32".into(), None),
            ("../../../research/ar-sample.f32".into(), Some("ar")),
            ("../../../research/it-sample.f32".into(), None),
            ("../../../research/it-sample.f32".into(), Some("it")),
            ("../../../research/it-sample.f32".into(), Some("auto-translate")),
        ]
    };
    for (file, lang) in cases {
        let file = file.as_str();
        let samples = read_f32le(&PathBuf::from(file).display().to_string());
        let t = std::time::Instant::now();
        match eng
            .transcribe_audio_with_confidence(samples, lang.map(|s| s.to_string()))
            .await
        {
            Ok((text, conf, partial)) => println!(
                "\n=== {} lang={:?} (conf={:.2}, partial={}, {:?}) ===\n{}",
                file,
                lang,
                conf,
                partial,
                t.elapsed(),
                text
            ),
            Err(e) => println!("\n=== {} lang={:?} FAILED: {} ===", file, lang, e),
        }
    }
    Ok(())
}
