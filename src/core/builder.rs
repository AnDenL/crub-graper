use super::scanner::{discover_sources, TranslationUnit};
use crate::config::{CompilerType, Package, load_manifest};
use colored::*;
use futures::future::join_all;
use petgraph::algo::{tarjan_scc, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;
use indicatif::{ProgressBar, ProgressStyle};

struct BuildContext {
    compiler: CompilerType,
    standard: String,
    flags: Vec<String>,
    include_dirs: Vec<String>,
    obj_dir: PathBuf,
    pb: ProgressBar,
}

#[derive(Serialize)]
struct CompileCommand {
    directory: String,
    arguments: Vec<String>,
    file: String,
    output: String,
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

    let order = match toposort(&graph, None) {
        Ok(ord) => ord,
        Err(_) => {
            let sccs = tarjan_scc(&graph);
            let cycle = sccs.iter()
                .find(|scc| scc.len() > 1 || graph.contains_edge(scc[0], scc[0]))
                .expect("Toposort failed but no SCC found");

            let mut cycle_info = Vec::new();
            for &node_idx in cycle {
                let unit_idx = graph[node_idx]; 
                let unit = &units[unit_idx];
                
                let name = unit.exported_module.clone().unwrap_or_else(|| "main".to_string());
                let path = unit.path.display().to_string();
                cycle_info.push(format!("{} ({})", name.bright_cyan(), path.dimmed()));
            }

            let first = cycle_info[0].clone();
            cycle_info.push(first);

            let error_msg = format!(
                "\n{}\n{}\n",
                "Circular dependency detected!".bright_red().bold(),
                format_cycle_diagram(&cycle_info)
            );
            return Err(error_msg);
        }
    };

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

    if let Err(e) = write_compdb(pkg, &units, &deep_hashes, &node_indices, &obj_dir) {
        println!("{}", format!("⚠️ Warning: Failed to generate compile_commands.json: {}", e).yellow());
    }

    let total_nodes = graph.node_count() as u64;
    let pb = ProgressBar::new(total_nodes);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan.bold} [{bar:40.cyan/blue}] {pos}/{len} ({percent}%) {msg:.yellow}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-")
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
                    pb.inc(1); // Оновлюємо прогрес-бар
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

    pb.finish_and_clear();
    println!("{}", "Compiling finished. Linking targets...".bright_cyan());

    if manifest.bin.is_empty() {
        println!("{}", "⚠️ Compiled successfully, but no [[bin]] targets found for linking.".yellow());
    }

    // Збираємо точки входу (entry_nodes) для всіх бінарників, щоб розділити їх
    let mut entry_nodes = HashMap::new();
    for bin in &manifest.bin {
        let clean_bin_path = bin.path.trim_start_matches('/');
        let target_path = PathBuf::from(clean_bin_path);
        
        if let Some((_, &node)) = path_to_node.iter().find(|(path, _)| path.ends_with(&target_path)) {
            entry_nodes.insert(bin.name.clone(), node);
        }
    }

    for bin in &manifest.bin {
        let out_path = build_dir.join(&bin.name);
        println!("{:>12} {}", "Linking".magenta().bold(), bin.name);

        if !entry_nodes.contains_key(&bin.name) {
            return Err(format!("Main file '{}' for target '{}' not found in source directory.", bin.path, bin.name));
        };

        let mut required_objects = Vec::new();
        
        // Нова логіка: лінкуємо всі об'єктні файли, ОКРІМ тих, які є entry_point для ІНШИХ бінарників
        for (node, obj_path) in &obj_files_map {
            let is_other_entry = entry_nodes.iter().any(|(name, &n)| name != &bin.name && n == *node);
            if !is_other_entry {
                required_objects.push(obj_path.clone());
            }
        }

        let mut cmd = Command::new(&pkg.compiler.as_string());
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
            println!("{}\n", stderr.dimmed());
            return Err(format!("Failed to link target '{}'", bin.name));
        }
    }

    println!("{:>12} project!", "Finished".bright_green().bold());
    Ok(())
}

pub async fn export_compdb(manifest_path: &str) -> Result<(), String> {
    let manifest = load_manifest(manifest_path)?;
    let pkg = &manifest.package;

    let build_dir = PathBuf::from(&pkg.out_dir);
    let obj_dir = build_dir.join("obj");

    let units = discover_sources(&pkg.source_dir, pkg);
    if units.is_empty() {
        return Err("No C++ source files found in the source directory.".to_string());
    }

    let mut graph = DiGraph::<usize, ()>::new();
    let mut module_to_node = HashMap::new();
    let mut node_indices = Vec::new();

    for (i, unit) in units.iter().enumerate() {
        let idx = graph.add_node(i);
        node_indices.push(idx);
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

    let order = toposort(&graph, None).map_err(|_| "🔄 Cyclic dependency detected in your C++ modules!".to_string())?;

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

    write_compdb(pkg, &units, &deep_hashes, &node_indices, &obj_dir)?;
    
    println!("{}", "compile_commands.json generated successfully!".bright_green());
    Ok(())
}

fn write_compdb(
    pkg: &Package,
    units: &[TranslationUnit],
    deep_hashes: &HashMap<NodeIndex, String>,
    node_indices: &[NodeIndex],
    obj_dir: &Path,
) -> Result<(), String> {
    let current_dir_path = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let current_dir_str = current_dir_path.to_string_lossy().to_string();

    let mut commands = Vec::new();

    for (i, unit) in units.iter().enumerate() {
        let node_idx = node_indices[i];
        let deep_hash = &deep_hashes[&node_idx];
        
        let safe_name = unit.path.file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("Can't get file name for path: {:?}", unit.path))?
            .replace('.', "_");

        let hash_prefix = &deep_hash[0..8];
        let obj_name = format!("{}_{}.o", safe_name, hash_prefix);
        
        let abs_file = fs::canonicalize(&unit.path)
            .unwrap_or_else(|_| current_dir_path.join(&unit.path));
        let abs_file_str = abs_file.to_string_lossy().to_string();

        let abs_output = current_dir_path.join(obj_dir).join(&obj_name);
        let abs_output_str = abs_output.to_string_lossy().to_string();

        let mut args = vec![
            pkg.compiler.as_string(),
            pkg.standard.clone(),
            "-c".to_string(),
        ];
        
        args.extend(pkg.flags.clone());
        
        for inc in &pkg.include_dirs {
            let abs_inc = current_dir_path.join(inc);
            args.push(format!("-I{}", abs_inc.to_string_lossy()));
        }
        let abs_obj_dir = current_dir_path.join(obj_dir);
        args.extend(pkg.compiler.get_flags(&abs_obj_dir, &unit));

        args.push(abs_file_str.clone());
        args.push("-o".to_string());
        args.push(abs_output_str.clone());

        commands.push(CompileCommand {
            directory: current_dir_str.clone(),
            arguments: args,
            file: abs_file_str,
            output: abs_output_str, 
        });
    }

    let json = serde_json::to_string_pretty(&commands)
        .map_err(|e| format!("Failed to serialize compile_commands.json: {}", e))?;
    
    fs::write(current_dir_path.join("compile_commands.json"), json)
        .map_err(|e| format!("Failed to write compile_commands.json: {}", e))?;

    Ok(())
}

async fn compile_unit(
    ctx: Arc<BuildContext>,
    unit: TranslationUnit,
    deep_hash: String,
) -> Result<PathBuf, String> {
    let safe_name = unit.path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("Can't get file name for path: {:?}", unit.path))?
        .replace('.', "_");
    let hash_prefix = &deep_hash[0..8];
    let obj_name = format!("{}_{}.o", safe_name, hash_prefix);
    
    let obj_path = ctx.obj_dir.join(&obj_name);
    let cache_file = ctx.obj_dir.join(format!("{}.hash", safe_name));

    let is_cached = if let Ok(cached_hash) = fs::read_to_string(&cache_file) {
        let mut valid = obj_path.exists();
        match ctx.compiler {
            CompilerType::Clang => {
                if let Some(mod_name) = &unit.exported_module {
                    let pcm_path = ctx.obj_dir.join(format!("{}.pcm", mod_name));
                    valid = valid && pcm_path.exists();
                }
            }
            _ => {},
        }
        cached_hash == deep_hash && valid
    } else {
        false
    };

    if is_cached {
        ctx.pb.println(format!("{:>12} {}", "Cached".bright_blue().bold(), unit.path.display()));
        return Ok(obj_path);
    }

    ctx.pb.println(format!("{:>12} {}", "Compiling".green().bold(), unit.path.display()));

    let mut cmd = Command::new(&ctx.compiler.as_string());
    cmd.arg(&ctx.standard).arg("-c");

    for flag in &ctx.flags {
        cmd.arg(flag);
    }

    for inc_dir in &ctx.include_dirs {
        cmd.arg(format!("-I{}", inc_dir));
    }

    let module_flags = ctx.compiler.get_flags(&ctx.obj_dir, &unit);
    cmd.args(module_flags);

    cmd.arg(&unit.path).arg("-o").arg(&obj_path);

    let output = cmd.output().await.map_err(|e| format!("Failed to spawn compiler: {}", e))?;

    if output.status.success() {
        fs::write(cache_file, &deep_hash).map_err(|e| e.to_string())?;
        Ok(obj_path)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        ctx.pb.println(format!("\n{} {}", "❌ Error compiling".red().bold(), unit.path.display()));
        ctx.pb.println(format!("{}", stderr));
        Err(format!("Compilation aborted for {}", unit.path.display()))
    }
}

fn format_cycle_diagram(path: &[String]) -> String {
    let mut diagram = String::new();
    let indent = "      ";
    
    for (i, module) in path.iter().enumerate() {
        if i == 0 {
            diagram.push_str(&format!("{}┌──→ {}\n", indent, module.bright_cyan()));
        } else if i == path.len() - 1 {
            diagram.push_str(&format!("{}└─── {}\n", indent, module.bright_cyan()));
        } else {
            diagram.push_str(&format!("{}│    {}\n", indent, module.bright_cyan()));
        }
    }
    diagram
}