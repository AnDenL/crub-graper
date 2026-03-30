use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Deserialize, Debug, Clone)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub bin: Vec<BinTarget>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Package {
    //#[allow(unused)]
    //pub name: String, I don't think its neaded
    #[allow(unused)]
    pub version: Option<String>,
    pub compiler: String,
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
    pub path: String, // Main file, e.g., src/main.cpp
}

pub fn load_manifest<P: AsRef<Path>>(path: P) -> Result<Manifest, String> {
    let content = fs::read_to_string(path)
        .map_err(|_| "Configuration file (Crub.toml) not found.".to_string())?;
    
    toml::from_str(&content)
        .map_err(|e| format!("Failed to parse Crub.toml: {}", e))
}