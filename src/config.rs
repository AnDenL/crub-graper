use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::core::scanner::TranslationUnit;

#[derive(Deserialize, Debug, Clone)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub bin: Vec<BinTarget>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Package {
    #[allow(unused)]
    pub version: Option<String>,
    pub compiler: CompilerType,
    pub standard: String,
    pub source_dir: String,
    pub out_dir: String,
    
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub lib_dirs: Vec<String>,
    #[serde(default)]
    pub libs: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BinTarget {
    pub name: String,
    pub path: String,
}

pub fn load_manifest<P: AsRef<Path>>(path: P) -> Result<Manifest, String> {
    let content = fs::read_to_string(path)
        .map_err(|_| "Configuration file (Crub.toml) not found.".to_string())?;
    
    toml::from_str(&content)
        .map_err(|e| format!("Failed to parse Crub.toml: {}", e))
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum CompilerType {
    Clang,
    Gcc,
    #[serde(untagged)]
    Custom(String),
}

impl CompilerType {
    pub fn as_str(&self) -> &str {
        match self {
            CompilerType::Clang => "clang++",
            CompilerType::Gcc => "g++",
            CompilerType::Custom(cmd) => cmd,
        }
    }

    pub fn as_string(&self) -> String {
        self.as_str().to_string()
    }

    pub fn get_flags(&self, obj_dir: &std::path::Path, unit: &TranslationUnit) -> Vec<String> {
        match self {
            CompilerType::Clang => {
                let mut args = Vec::new();
                args.push(format!("-fprebuilt-module-path={}", obj_dir.to_string_lossy()));
                
                if let Some(mod_name) = &unit.exported_module {
                    let pcm_path = obj_dir.join(format!("{}.pcm", mod_name));
                    args.push(format!("-fmodule-output={}", pcm_path.to_string_lossy()));
                    
                    let ext = unit.path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext != "cppm" && ext != "ixx" {
                        args.push("-x".to_string());
                        args.push("c++-module".to_string());
                    }
                }
                args
            },
            _=> vec!["-fmodules-ts".to_string()],
        }
    }
}

pub fn default_with_name(name: &String) -> String {
format!(r#"[package]
compiler = "clang++"
standard = "-std=c++26"
source_dir = "./src" 
out_dir = "./build"

# flags = []
# include_dirs = []
# lib_dirs = []
# libs = []

[[bin]]
name = "{}"
path = "src/main.cpp""#, name)
}