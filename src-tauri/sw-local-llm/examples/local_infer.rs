use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;
use sw_local_llm::{
    default_llm_dir, download_model_files, engine::LocalLlmEngine, model_dir, model_ready, resolve,
    InferenceParams, SidecarPaths,
};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let id_or_repo = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Qwen/Qwen2.5-0.5B-Instruct-GGUF".to_string());
    let prompt = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "In one short sentence, what is a transformer in machine learning?".to_string());

    let entry = resolve(&id_or_repo).ok_or_else(|| format!("could not resolve model '{id_or_repo}'"))?;
    let base = default_llm_dir();
    let dir = model_dir(&base, &entry);

    eprintln!("model:   {} ({})", entry.label, entry.repo_id);
    eprintln!("kind:    {:?}", entry.kind);
    eprintln!("dir:     {}", dir.display());

    if model_ready(&base, &entry) {
        eprintln!("status:  already downloaded, skipping fetch");
    } else {
        eprintln!("status:  downloading...");
        let started = Instant::now();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        download_model_files(&base, &entry, None, &cancel, |p| {
            if p.bytes_total > 0 {
                let pct = (p.bytes_downloaded as f64 / p.bytes_total as f64 * 100.0) as u64;
                eprint!(
                    "\r  [{}/{}] {} {}%        ",
                    p.files_completed, p.files_total, p.file_name, pct
                );
                let _ = std::io::stderr().flush();
            }
        })
        .await?;
        eprintln!("\nstatus:  downloaded in {:?}", started.elapsed());
    }

    let llama_dir = PathBuf::from(
        std::env::var("SW_LLAMA_DIR")
            .map_err(|_| "set SW_LLAMA_DIR to the folder holding llama-server and its dylibs")?,
    );
    let sidecar = SidecarPaths {
        server_bin: llama_dir.join("llama-server"),
        lib_dir: llama_dir,
    };

    eprintln!("starting llama.cpp sidecar...");
    let load_started = Instant::now();
    let engine = LocalLlmEngine::load(&sidecar, &dir, &entry).await?;
    eprintln!("ready in {:?}", load_started.elapsed());

    eprintln!("\nPROMPT: {prompt}\n--- generating ---");
    let params = InferenceParams {
        max_tokens: 128,
        ..InferenceParams::default()
    };
    let gen_started = Instant::now();
    let full = engine
        .infer(
            Some("You are a concise, helpful assistant."),
            &prompt,
            &params,
            |chunk| {
                if !chunk.done {
                    print!("{}", chunk.text);
                    let _ = std::io::stdout().flush();
                }
            },
        )
        .await?;
    println!("\n--- done in {:?} ---", gen_started.elapsed());

    if full.trim().is_empty() {
        return Err("inference produced empty output".into());
    }
    eprintln!("OK: generated {} chars", full.len());
    Ok(())
}
