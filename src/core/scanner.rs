use crate::config::Package;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct TranslationUnit {
    pub path: PathBuf,
    pub exported_module: Option<String>,
    pub imports: Vec<String>,
    pub base_hash: String, // Hash of the file itself
}

fn compute_base_hash(content: &str, config: &Package) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hasher.update(&config.compiler);
    hasher.update(&config.standard);
    
    // Include flags and include directories in the hash 
    // so that changing them triggers a recompilation
    for flag in &config.flags {
        hasher.update(flag);
    }
    for inc_dir in &config.include_dirs {
        hasher.update(inc_dir);
    }
    
    hex::encode(hasher.finalize())
}

pub fn scan_file(path: &Path, config: &Package) -> Option<TranslationUnit> {
    let content = fs::read_to_string(path).ok()?;

    let re_export = Regex::new(r"(?m)^\s*export\s+module\s+([a-zA-Z0-9_\.:]+)\s*;").unwrap();
    let re_module = Regex::new(r"(?m)^\s*module\s+([a-zA-Z0-9_\.:]+)\s*;").unwrap();
    let re_import = Regex::new(r"(?m)^\s*import\s+([a-zA-Z0-9_\.:]+)\s*;").unwrap();

    let exported_module = re_export.captures(&content)
        .or_else(|| re_module.captures(&content))
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string());

    let imports = re_import
        .captures_iter(&content)
        .map(|cap| cap[1].to_string())
        .collect();

    Some(TranslationUnit {
        path: path.to_path_buf(),
        exported_module,
        imports,
        base_hash: compute_base_hash(&content, config),
    })
}

pub fn discover_sources(source_dir: &str, config: &Package) -> Vec<TranslationUnit> {
    let mut units = Vec::new();
    for entry in walkdir::WalkDir::new(source_dir).into_iter().filter_map(|e| e.ok()) {
        if let Some(ext) = entry.path().extension() {
            // Added .ixx support for MSVC/Clang module interfaces
            if ext == "cppm" || ext == "cpp" || ext == "cxx" || ext == "cc" || ext == "ixx" {
                if let Some(unit) = scan_file(entry.path(), config) {
                    units.push(unit);
                }
            }
        }
    }
    units
}