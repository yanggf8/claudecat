//! Manifest detection: package.json / Cargo.toml / pyproject.toml / go.mod / requirements.txt
use crate::model::{DepGroup, ProjectMeta};
use std::path::Path;

pub fn detect_project_meta(root: &Path) -> (ProjectMeta, Vec<DepGroup>) {
    let dir_name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let mut deps: Vec<DepGroup> = Vec::new();
    let mut meta = ProjectMeta::default();

    let has = |name: &str| root.join(name).is_file();

    if has("package.json") {
        if let Some((m, d)) = parse_package_json(&root.join("package.json")) {
            meta = m;
            deps.push(d);
            meta.project_type = "Node.js application/library".into();
            meta.package_manager = "npm".into();
        }
    }
    if has("Cargo.toml") {
        if let Some((m, d)) = parse_cargo_toml(&root.join("Cargo.toml")) {
            merge_meta(&mut meta, m);
            deps.push(d);
            meta.package_manager = "cargo".into();
        }
    }
    if has("pyproject.toml") {
        if let Some((m, d)) = parse_pyproject(&root.join("pyproject.toml")) {
            merge_meta(&mut meta, m);
            deps.push(d);
            meta.package_manager = "uv/pip".into();
        }
    }
    if has("requirements.txt") {
        if let Some(d) = parse_requirements(&root.join("requirements.txt")) {
            deps.push(d);
        }
    }
    if has("go.mod") {
        if let Some(m) = parse_go_mod(&root.join("go.mod")) {
            merge_meta(&mut meta, m);
            meta.package_manager = "go modules".into();
        }
    }
    if has("Gemfile") {
        meta.project_type = "Ruby application/library".into();
        meta.package_manager = "bundler".into();
        meta.language = "Ruby".into();
    }
    if has("composer.json") {
        meta.project_type = "PHP application/library".into();
        meta.package_manager = "composer".into();
        meta.language = "PHP".into();
    }

    // merge duplicate ecosystems (e.g. pyproject + requirements.txt)
    let mut merged: Vec<DepGroup> = Vec::new();
    for g in deps {
        if let Some(existing) = merged.iter_mut().find(|e| e.ecosystem == g.ecosystem) {
            existing.deps.extend(g.deps);
            existing.deps.sort();
            existing.deps.dedup();
        } else {
            merged.push(g);
        }
    }
    let deps = merged;
    if meta.name.is_empty() {
        meta.name = dir_name;
    }
    infer_framework(&mut meta, &deps);

    // Entry points fallback
    if meta.entry_points.is_empty() {
        let mut seen = std::collections::HashSet::new();
    meta.entry_points.retain(|e| seen.insert(e.clone()));
    for guess in [
            "src/main.rs",
            "main.py",
            "src/main.py",
            "__main__.py",
            "main.go",
            "src/main.go",
            "index.js",
            "index.ts",
            "src/index.js",
            "src/index.ts",
            "app.js",
            "app.py",
            "server.js",
            "src/server.js",
        ] {
            if root.join(guess).is_file() {
                meta.entry_points.push(guess.to_string());
            }
        }
    }
    (meta, deps)
}

fn merge_meta(base: &mut ProjectMeta, src: ProjectMeta) {
    if base.name.is_empty() {
        base.name = src.name;
    }
    if base.project_type.is_empty() {
        base.project_type = src.project_type;
    }
    if base.language.is_empty() {
        base.language = src.language;
    }
    if base.framework.is_empty() {
        base.framework = src.framework;
    }
    if base.entry_points.is_empty() {
        base.entry_points = src.entry_points;
    }
    if base.run_command.is_none() {
        base.run_command = src.run_command;
    }
    if base.build_command.is_none() {
        base.build_command = src.build_command;
    }
    base.scripts.extend(src.scripts);
}

fn parse_package_json(path: &Path) -> Option<(ProjectMeta, DepGroup)> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut meta = ProjectMeta::default();
    meta.project_type = "Node.js application/library".into();
    meta.package_manager = "npm".into();
    meta.name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").into();
    meta.language = "TypeScript/JavaScript".into();

    if let Some(bins) = v.get("bin") {
        let mut entries = vec![];
        if let Some(s) = bins.as_str() {
            entries.push(s.to_string());
        } else if let Some(map) = bins.as_object() {
            for (k, p) in map {
                if let Some(p) = p.as_str() {
                    entries.push(format!("{k} -> {p}"));
                }
            }
        }
        meta.entry_points.extend(entries);
    }
    if let Some(main) = v.get("main").and_then(|x| x.as_str()) {
        meta.entry_points.push(main.to_string());
    }
    if let Some(scripts) = v.get("scripts").and_then(|x| x.as_object()) {
        for (k, val) in scripts {
            if let Some(val) = val.as_str() {
                let key = k.clone();
                let value = val.to_string();
                match key.as_str() {
                    "start" => { meta.run_command = Some(format!("npm start ({value})")); }
                    "dev" => { meta.run_command = meta.run_command.clone().or(Some(format!("npm run dev ({value})"))); }
                    "build" => { meta.build_command = Some(format!("npm run build ({value})")); }
                    _ => {}
                }
                meta.scripts.insert(key, value);
            }
        }
    }

    let mut deps: Vec<String> = Vec::new();
    for key in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(map) = v.get(key).and_then(|x| x.as_object()) {
            for name in map.keys() {
                deps.push(name.clone());
            }
        }
    }
    deps.sort();
    deps.dedup();
    let group = DepGroup { ecosystem: "npm".into(), deps };
    Some((meta, group))
}

fn parse_cargo_toml(path: &Path) -> Option<(ProjectMeta, DepGroup)> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    let mut meta = ProjectMeta::default();
    meta.project_type = "Rust application/library".into();
    meta.package_manager = "cargo".into();
    meta.language = "Rust".into();
    if let Some(pkg) = v.get("package") {
        meta.name = pkg.get("name").and_then(|x| x.as_str()).unwrap_or("").into();
        if let Some(bin_path) = pkg.get("default-run").and_then(|x| x.as_str()) {
            meta.entry_points.push(bin_path.to_string());
        }
    }
    if let Some(bins) = v.get("bin").and_then(|x| x.as_array()) {
        for b in bins {
            let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let path = b.get("path").and_then(|x| x.as_str()).unwrap_or("");
            meta.entry_points.push(format!("{name} ({path})"));
        }
    }
    if path.parent().map(|p| p.join("src/main.rs").is_file()).unwrap_or(false) {
        meta.entry_points.push("src/main.rs".into());
    }
    meta.run_command = Some("cargo run".into());
    meta.build_command = Some("cargo build".into());

    let mut deps: Vec<String> = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(map) = v.get(section).and_then(|x| x.as_table()) {
            for name in map.keys() {
                deps.push(name.clone());
            }
        }
    }
    deps.sort();
    deps.dedup();
    let group = DepGroup { ecosystem: "crates.io".into(), deps };
    Some((meta, group))
}

fn parse_pyproject(path: &Path) -> Option<(ProjectMeta, DepGroup)> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    let mut meta = ProjectMeta::default();
    meta.project_type = "Python application/library".into();
    meta.package_manager = "uv/pip".into();
    meta.language = "Python".into();
    if let Some(proj) = v.get("project") {
        meta.name = proj.get("name").and_then(|x| x.as_str()).unwrap_or("").into();
        if let Some(scripts) = proj.get("scripts").and_then(|x| x.as_table()) {
            for (k, val) in scripts {
                if let Some(v) = val.as_str() {
                    meta.entry_points.push(format!("{k} -> {v}"));
                }
            }
        }
    }
    let mut deps: Vec<String> = Vec::new();
    if let Some(list) = v
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_array())
    {
        for item in list {
            if let Some(s) = item.as_str() {
                // "package>=1.0" -> "package"
                let name = s
                    .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
                    .next()
                    .unwrap_or(s)
                    .to_string();
                deps.push(name);
            }
        }
    }
    deps.sort();
    deps.dedup();
    let group = DepGroup { ecosystem: "PyPI".into(), deps };
    Some((meta, group))
}

fn parse_requirements(path: &Path) -> Option<DepGroup> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut deps = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }
        let name = line
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
            .next()
            .unwrap_or(line)
            .to_string();
        if !name.is_empty() {
            deps.push(name);
        }
    }
    deps.sort();
    deps.dedup();
    Some(DepGroup { ecosystem: "PyPI".into(), deps })
}

fn parse_go_mod(path: &Path) -> Option<ProjectMeta> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut meta = ProjectMeta::default();
    meta.project_type = "Go application/library".into();
    meta.package_manager = "go modules".into();
    meta.language = "Go".into();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("module ") {
            meta.name = rest.trim().to_string();
        } else if let Some(rest) = line.trim().strip_prefix("//") {
            // ignore comments
            let _ = rest;
        }
    }
    if path.parent().map(|p| p.join("main.go").is_file()).unwrap_or(false) {
        meta.entry_points.push("main.go".into());
    }
    meta.run_command = Some("go run .".into());
    meta.build_command = Some("go build".into());
    Some(meta)
}

fn infer_framework(meta: &mut ProjectMeta, deps: &[DepGroup]) {
    // Framework inference is dependency-driven (factual, from manifests), not heuristic scanning.
    if !meta.framework.is_empty() {
        return;
    }
    let known: &[(&str, &[&str])] = &[
        ("Express.js", &["express"]),
        ("Fastify", &["fastify"]),
        ("NestJS", &["@nestjs/core", "nest"]),
        ("Next.js", &["next"]),
        ("Nuxt", &["nuxt"]),
        ("Koa", &["koa"]),
        ("Hono", &["hono"]),
        ("React", &["react"]),
        ("Vue", &["vue"]),
        ("Svelte", &["svelte"]),
        ("Angular", &["@angular/core"]),
        ("Astro", &["astro"]),
        ("Rocket (Rust)", &["rocket"]),
        ("Axum (Rust)", &["axum"]),
        ("Actix-web (Rust)", &["actix-web"]),
        ("Tokio (Rust)", &["tokio"]),
        ("Django", &["django"]),
        ("Flask", &["flask"]),
        ("FastAPI", &["fastapi"]),
        ("Gin (Go)", &["gin-gonic/gin"]),
        ("Echo (Go)", &["labstack/echo"]),
        ("Chi (Go)", &["go-chi/chi"]),
        ("Rails", &["rails"]),
        ("Symfony (PHP)", &["symfony/console"]),
        ("Laravel (PHP)", &["laravel/framework"]),
    ];
    let mut all: Vec<&str> = Vec::new();
    for g in deps {
        for d in &g.deps {
            let low = d.to_lowercase();
            for (frame, needles) in known {
                if needles.iter().any(|n| low == *n || low.starts_with(&format!("{n} ")) || low.contains(&format!("/{n}"))) {
                    all.push(frame);
                }
            }
        }
    }
    all.sort();
    all.dedup();
    meta.framework = all.join(", ");
}
