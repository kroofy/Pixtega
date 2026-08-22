//! Process bootstrap: load configuration, initialize libvips, verify the
//! enabled encoders, build the Source registry, and serve.

use std::path::PathBuf;
use std::process::ExitCode;

use pixtega::{app, config, processor};

fn config_source() -> Result<(String, PathBuf), String> {
    // Precedence: CLI path, then CONFIG_FILE, then inline CONFIG.
    let mut args = std::env::args().skip(1);
    if let Some(path) = args.next() {
        return Ok(("file".to_string(), PathBuf::from(path)));
    }
    if let Ok(path) = std::env::var("CONFIG_FILE") {
        return Ok(("file".to_string(), PathBuf::from(path)));
    }
    if std::env::var("CONFIG").is_ok() {
        return Ok(("inline".to_string(), PathBuf::new()));
    }
    Err("no configuration: pass a TOML path, or set CONFIG_FILE or CONFIG".to_string())
}

fn main() -> ExitCode {
    let loaded = match config_source() {
        Ok((kind, path)) => {
            if kind == "inline" {
                let text = std::env::var("CONFIG").expect("checked above");
                let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                config::load_from_str(&text, &base)
            } else {
                config::load_from_file(&path)
            }
        }
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    let cfg = match loaded {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    processor::init_vips();
    let enabled: Vec<_> = cfg.formats.keys().copied().collect();
    if let Err(err) = processor::verify_encoders(&enabled) {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    match runtime.block_on(app::run(cfg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fatal: {err}");
            ExitCode::FAILURE
        }
    }
}
