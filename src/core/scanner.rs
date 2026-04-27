use crate::config::Package;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static RE_EXPORT: OnceLock<Regex> = OnceLock::new();
static RE_MODULE: OnceLock<Regex> = OnceLock::new();
static RE_IMPORT: OnceLock<Regex> = OnceLock::new();
static RE_STRIP: OnceLock<Regex> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct TranslationUnit {
    pub path: PathBuf,
    pub exported_module: Option<String>,
    pub imports: Vec<String>,
    pub base_hash: String, 
}

fn compute_base_hash(content: &str, config: &Package) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hasher.update(config.compiler.as_str());
    hasher.update(&config.standard);
    
    for flag in &config.flags {
        hasher.update(flag);
    }
    for inc_dir in &config.include_dirs {
        hasher.update(inc_dir);
    }
    
    hex::encode(hasher.finalize())
}

fn strip_comments(text: &str) -> String {
    let re = RE_STRIP.get_or_init(|| 
        Regex::new(r"//[^\n]*|(?s)/\*.*?\*/").expect("Invalid Strip Regex")
    );
    re.replace_all(text, " ").to_string()
}

pub fn scan_file(path: &Path, config: &Package) -> Option<TranslationUnit> {
    let raw_content = fs::read_to_string(path).ok()?;

    let content = strip_comments(&raw_content);

    let re_export = RE_EXPORT.get_or_init(|| Regex::new(r"(?m)^\s*export\s+module\s+([a-zA-Z0-9_\.:]+)\s*;").expect("Invalid Regex"));
    let re_module = RE_MODULE.get_or_init(|| Regex::new(r"(?m)^\s*module\s+([a-zA-Z0-9_\.:]+)\s*;").expect("Invalid Regex"));
    let re_import = RE_IMPORT.get_or_init(|| Regex::new(r"(?m)^\s*import\s+([a-zA-Z0-9_\.:]+)\s*;").expect("Invalid Regex"));

    let exported_module = re_export.captures(&content)
        .or_else(|| re_module.captures(&content))
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string());

    let imports = re_import
        .captures_iter(&content)
        .filter_map(|cap| cap.get(1)) 
        .map(|m| m.as_str().to_string())
        .collect();

    Some(TranslationUnit {
        path: path.to_path_buf(),
        exported_module,
        imports,
        base_hash: compute_base_hash(&raw_content, config),
    })
}

pub fn discover_sources(source_dir: &str, config: &Package) -> Vec<TranslationUnit> {
    let mut units = Vec::new();
    for entry in walkdir::WalkDir::new(source_dir).into_iter().filter_map(|e| e.ok()) {
        if let Some(ext) = entry.path().extension() {
            if ext == "cppm" || ext == "cpp" || ext == "cxx" || ext == "cc" || ext == "ixx" {
                if let Some(unit) = scan_file(entry.path(), config) {
                    units.push(unit);
                }
            }
        }
    }
    units
}