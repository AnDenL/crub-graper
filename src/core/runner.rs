use crate::config::load_manifest;
use std::path::PathBuf;
use tokio::process::Command;

pub async fn run_target(manifest_path: &str, target_name: Option<&str>) -> Result<(), String> {
    let manifest = load_manifest(manifest_path)?;
    
    if manifest.bin.is_empty() {
        return Err("No [[bin]] targets found in configuration!".to_string());
    }

    let bin_target = if let Some(name) = target_name {
        manifest.bin.iter().find(|b| b.name == name)
            .ok_or_else(|| format!("Target '{}' not found", name))?
    } else {
        &manifest.bin[0]
    };

    let bin_path = PathBuf::from(&manifest.package.out_dir).join(&bin_target.name);

    if !bin_path.exists() {
        return Err("Executable not found. Did the build fail?".to_string());
    }

    let mut cmd = Command::new(&bin_path);    
    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn process: {}", e))?;
    let status = child.wait().await.map_err(|e| format!("Process wait error: {}", e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Exit code: {}", status.code().unwrap_or(-1)))
    }
}