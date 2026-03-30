use super::scanner::{discover_sources, TranslationUnit};
use crate::config::{load_manifest};
use colored::*;
use futures::future::join_all;
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::Reversed;
use petgraph::Direction;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use indicatif::{ProgressBar, ProgressStyle};

// Context passed to worker threads
struct BuildContext {
    compiler: String,
    standard: String,
    flags: Vec<String>,
    include_dirs: Vec<String>,
    obj_dir: PathBuf,
    pb: ProgressBar, // Progress bar for thread-safe console output
}

pub async fn build_project(manifest_path: &str, _target_name: Option<&str>) -> Result<(), String> {
    let manifest = load_manifest(manifest_path)?;
    let pkg = &manifest.package;

    let build_dir = PathBuf::from(&pkg.out_dir);
    let obj_dir = build_dir.join("obj");
    fs::create_dir_all(&obj_dir).map_err(|e| format!("Failed to create obj dir: {}", e))?;

    let units = discover_sources(&pkg.source_dir, pkg);
    if units.is_empty() {
        return Err("No C++ source files found in the source directory.".to_string());
    }

    let mut graph = DiGraph::<usize, ()>::new();
    let mut module_to_node = HashMap::new();
    let mut path_to_node = HashMap::new();
    let mut node_indices = Vec::new();

    for (i, unit) in units.iter().enumerate() {
        let idx = graph.add_node(i);
        node_indices.push(idx);
        path_to_node.insert(unit.path.clone(), idx);
        if let Some(mod_name) = &unit.exported_module {
            module_to_node.insert(mod_name.clone(), idx);
        }
    }

    for (i, unit) in units.iter().enumerate() {
        for imp in &unit.imports {
            if let Some(&target_idx) = module_to_node.get(imp) {
                graph.add_edge(target_idx, node_indices[i], ());
            }
        }
    }

    let order = toposort(&graph, None).map_err(|_| {"🔄 Cyclic dependency detected in your C++ modules!"})?;

    let mut deep_hashes: HashMap<NodeIndex, String> = HashMap::new();
    for &node_idx in &order {
        let unit_idx = graph[node_idx];
        let mut hasher = Sha256::new();
        hasher.update(&units[unit_idx].base_hash);

        for neighbor in graph.neighbors_directed(node_idx, Direction::Incoming) {
            if let Some(dep_hash) = deep_hashes.get(&neighbor) {
                hasher.update(dep_hash);
            }
        }
        deep_hashes.insert(node_idx, hex::encode(hasher.finalize()));
    }

    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(150));
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_strings(&[
                "🦀🔪           ❤️",
                "  🦀🔪        ❤️",
                "    🦀🔪       ❤️",
                "      🦀🔪    ❤️",
                "        🦀🔪   ❤️",
                "          🦀🔪 ❤️",
                "            🔪🦀💔",
                " ❤️          🔪🦀 ",
                "❤️          🔪🦀 ",
                " ❤️       🔪🦀   ",
                "❤️      🔪🦀     ",
                " ❤️   🔪🦀       ",
                "❤️  🔪🦀         ",
                "💔🔪🦀           ",
                "🦀🔪             ",
            ])
            .template("{spinner:.cyan.bold} {msg:.yellow}")
            .unwrap(),
    );
    pb.set_message("Compiling C++ chaos...");

    let build_ctx = Arc::new(BuildContext {
        compiler: pkg.compiler.clone(),
        standard: pkg.standard.clone(),
        flags: pkg.flags.clone(),
        include_dirs: pkg.include_dirs.clone(),
        obj_dir: obj_dir.clone(),
        pb: pb.clone(), 
    });

    pb.println(format!("{}", "🚀 Starting parallel build...".bright_cyan().bold()));

    let mut in_degrees: HashMap<NodeIndex, usize> = HashMap::new();
    for node in graph.node_indices() {
        in_degrees.insert(node, graph.edges_directed(node, Direction::Incoming).count());
    }

    let mut completed = HashSet::new();
    let mut obj_files_map: HashMap<NodeIndex, PathBuf> = HashMap::new();

    while completed.len() < graph.node_count() {
        let mut current_wave = Vec::new();

        for node in graph.node_indices() {
            if !completed.contains(&node) && in_degrees[&node] == 0 {
                current_wave.push(node);
            }
        }

        if current_wave.is_empty() {
            pb.finish_and_clear();
            return Err("Deadlock detected during parallel compilation!".to_string());
        }

        let mut tasks = Vec::new();

        for &node_idx in &current_wave {
            let unit = units[graph[node_idx]].clone();
            let deep_hash = deep_hashes[&node_idx].clone();
            let ctx = Arc::clone(&build_ctx);

            tasks.push(tokio::spawn(async move {
                compile_unit(ctx, unit, deep_hash).await.map(|obj_path| (node_idx, obj_path))
            }));
        }

        let wave_results = join_all(tasks).await;

        for res in wave_results {
            match res {
                Ok(Ok((node_idx, obj_path))) => {
                    obj_files_map.insert(node_idx, obj_path);
                    completed.insert(node_idx);
                    for neighbor in graph.neighbors_directed(node_idx, Direction::Outgoing) {
                        *in_degrees.get_mut(&neighbor).unwrap() -= 1;
                    }
                }
                Ok(Err(e)) => {
                    pb.finish_and_clear();
                    return Err(e);
                }
                Err(e) => {
                    pb.finish_and_clear();
                    return Err(format!("Task panicked: {}", e));
                }
            }
        }
    }

    pb.set_message("Linking final binaries...");

    if manifest.bin.is_empty() {
        pb.println(format!("{}", "⚠️ Compiled successfully, but no [[bin]] targets found for linking.".yellow()));
    }

    for bin in &manifest.bin {
        let out_path = build_dir.join(&bin.name);
        pb.println(format!("{:>12} {}", "Linking".magenta().bold(), bin.name));

        let clean_bin_path = bin.path.trim_start_matches('/');
        let target_path = PathBuf::from(clean_bin_path);

        let main_node_opt = path_to_node.iter().find_map(|(path, &node)| {
            if path.ends_with(&target_path) {
                Some(node)
            } else {
                None
            }
        });

        let Some(main_node) = main_node_opt else {
            pb.finish_and_clear();
            return Err(format!("Main file '{}' for target '{}' not found in source directory.", bin.path, bin.name));
        };

        let mut required_objects = Vec::new();
        let mut dfs = petgraph::visit::Dfs::new(Reversed(&graph), main_node);
        
        while let Some(nx) = dfs.next(Reversed(&graph)) {
            if let Some(obj_path) = obj_files_map.get(&nx) {
                required_objects.push(obj_path.clone());
            }
        }

        let mut cmd = Command::new(&pkg.compiler);
        cmd.arg(&pkg.standard);

        for flag in &pkg.flags {
            cmd.arg(flag);
        }

        for obj in &required_objects {
            cmd.arg(obj);
        }

        cmd.arg("-o").arg(&out_path);

        for lib_dir in &pkg.lib_dirs {
            cmd.arg(format!("-L{}", lib_dir));
        }

        for lib in &pkg.libs {
            cmd.arg(format!("-l{}", lib));
        }

        let output = cmd.output().await.map_err(|e| e.to_string())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            pb.println(format!("{}\n", stderr.dimmed()));
            pb.finish_and_clear();
            return Err(format!("Failed to link target '{}'", bin.name));
        }
    }

    // Clean up animation before final print
    pb.finish_and_clear();
    println!("{:>12} project!", "Finished".bright_green().bold());
    Ok(())
}

async fn compile_unit(
    ctx: Arc<BuildContext>,
    unit: TranslationUnit,
    deep_hash: String,
) -> Result<PathBuf, String> {
    let safe_name = unit.path.file_stem().unwrap().to_str().unwrap().replace('.', "_");
    let hash_prefix = &deep_hash[0..8];
    let obj_name = format!("{}_{}.o", safe_name, hash_prefix);
    
    let obj_path = ctx.obj_dir.join(&obj_name);
    let cache_file = ctx.obj_dir.join(format!("{}.hash", safe_name));

    let is_cached = if let Ok(cached_hash) = fs::read_to_string(&cache_file) {
        cached_hash == deep_hash && obj_path.exists()
    } else {
        false
    };

    if is_cached {
        // USE pb.println() to not overwrite the animation!
        ctx.pb.println(format!("{:>12} {}", "Cached".bright_blue().bold(), unit.path.display()));
        return Ok(obj_path);
    }

    ctx.pb.println(format!("{:>12} {}", "Compiling".green().bold(), unit.path.display()));

    let mut cmd = Command::new(&ctx.compiler);
    cmd.arg(&ctx.standard).arg("-c");

    for flag in &ctx.flags {
        cmd.arg(flag);
    }

    for inc_dir in &ctx.include_dirs {
        cmd.arg(format!("-I{}", inc_dir));
    }

    if ctx.compiler.contains("clang") {
        cmd.arg("-fprebuilt-module-path=.");
        if unit.exported_module.is_some() || unit.path.extension().unwrap_or_default() == "cppm" {
            cmd.arg("-Xclang").arg("-emit-module-interface");
        }
    } else {
        cmd.arg("-fmodules-ts");
    }

    cmd.arg(&unit.path).arg("-o").arg(&obj_path);

    let output = cmd.output().await.map_err(|e| format!("Failed to spawn compiler: {}", e))?;

    if output.status.success() {
        fs::write(cache_file, &deep_hash).unwrap();
        Ok(obj_path)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        ctx.pb.println(format!("\n{} {}", "❌ Error compiling".red().bold(), unit.path.display()));
        ctx.pb.println(format!("{}", stderr));
        Err(format!("Compilation aborted for {}", unit.path.display()))
    }
}