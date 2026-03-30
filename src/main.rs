mod cli;
mod config;
mod core;

use clap::Parser;
use cli::{Cli, Commands};
use colored::*;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Build { manifest, target } => {
            println!("{}", "🦀 Crub-graper: Building C++ project...".bright_green());
            if let Err(e) = core::builder::build_project(manifest, target.as_deref()).await {
                eprintln!("{} {}", "❌ Build failed:".red().bold(), e);
                std::process::exit(1);
            }
        }
        Commands::Run { manifest, target } => {
            if let Err(e) = core::builder::build_project(manifest, target.as_deref()).await {
                eprintln!("{} {}", "❌ Cannot run: build failed:".red().bold(), e);
                std::process::exit(1);
            }

            println!("{}", "Running target...".bright_cyan());
            if let Err(e) = core::runner::run_target(manifest, target.as_deref()).await {
                eprintln!("{} {}", "Program terminated unexpectedly:".red().bold(), e);
                std::process::exit(1);
            }
        }
    }
}