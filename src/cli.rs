use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "crub-graper", version = "0.1", about = "A Cargo-like build system for C++")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Build the project (compile + link)
    Build {
        #[arg(short, long, default_value = "Crub.toml")]
        manifest: String,
        
        /// Build a specific target (e.g., binary name)
        #[arg(short, long)]
        target: Option<String>,
    },
    /// Build and run the project
    Run {
        #[arg(short, long, default_value = "Crub.toml")]
        manifest: String,

        /// Run a specific target
        #[arg(short, long)]
        target: Option<String>,
    },
    /// Generate compile_commands.json for VS Code / clangd
    Compdb {
        #[arg(short, long, default_value = "Crub.toml")]
        manifest: String,
    },
    /// Generate Crub.toml
    Toml {
        #[arg(default_value = "my_app")]
        name: String,
    },
}