//! `zslide` — inspect, extract text from, and convert PowerPoint (.pptx) files.
//!
//! Note: opening is text-only (see `Presentation::open`), so `convert` reflects
//! extracted text, not a faithful re-render of the original deck.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use zavora_slide::{Presentation, RenderFormat};

#[derive(Parser)]
#[command(name = "zslide", about = "Inspect and convert PowerPoint (.pptx) files")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print a structural summary (slide count + size).
    Inspect { file: String },
    /// Extract plain text (markdown outline).
    Text { file: String },
    /// Convert to PDF, or a single slide to PNG/SVG.
    Convert {
        file: String,
        #[arg(short, long)]
        output: String,
        /// For PNG/SVG output, the 0-based slide index (default 0).
        #[arg(short, long, default_value_t = 0)]
        slide: usize,
    },
}

fn run() -> Result<(), String> {
    match Cli::parse().cmd {
        Cmd::Inspect { file } => {
            let p = Presentation::open(&file).map_err(|e| e.to_string())?;
            println!("slides: {}", p.slide_count());
        }
        Cmd::Text { file } => {
            let p = Presentation::open(&file).map_err(|e| e.to_string())?;
            print!("{}", p.to_markdown());
        }
        Cmd::Convert { file, output, slide } => {
            let p = Presentation::open(&file).map_err(|e| e.to_string())?;
            let lower = output.to_ascii_lowercase();
            if lower.ends_with(".pdf") {
                p.save_pdf(&output).map_err(|e| e.to_string())?;
            } else if lower.ends_with(".png") || lower.ends_with(".svg") {
                let fmt = if lower.ends_with(".svg") { RenderFormat::Svg } else { RenderFormat::Png };
                let bytes = p.render_slide(slide, fmt).map_err(|e| e.to_string())?;
                std::fs::write(&output, bytes).map_err(|e| e.to_string())?;
            } else {
                return Err("output must end in .pdf, .png, or .svg".into());
            }
            println!("wrote {output}");
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zslide: {e}");
            ExitCode::FAILURE
        }
    }
}
