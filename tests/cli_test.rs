mod common;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_claudecat")
}

#[test]
fn scan_json_is_valid_and_complete() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "package.json", r#"{"name":"webapp","scripts":{"start":"node server.js"},"dependencies":{"express":"^4"}}"#);
    common::write(&t.0, "server.js", "const express = require('express');\napp.get('/', (req, res) => res.json({}));\n");
    let out = Command::new(bin())
        .args(["scan", "--root", t.0.to_str().unwrap(), "--format", "json", "--top-files", "5"])
        .output()
        .expect("run scan");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(v["meta"]["framework"], "Express.js");
    assert_eq!(v["meta"]["name"], "webapp");
    assert!(v["key_files"].as_array().unwrap().iter().any(|f| f["path"].as_str().unwrap() == "server.js"));
}

#[test]
fn update_dry_run_does_not_write() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "Cargo.toml", "[package]\nname=\"demo\"\n");
    common::write(&t.0, "src/main.rs", "fn main() {}\n");
    let out = Command::new(bin())
        .args(["update", "--root", t.0.to_str().unwrap(), "--dry-run", "--top-files", "5"])
        .output()
        .expect("run update dry-run");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(!t.0.join("CLAUDE.md").exists(), "dry-run must not create CLAUDE.md");
    assert!(String::from_utf8_lossy(&out.stdout).contains("WOULD UPDATE"));
}

#[test]
fn update_writes_section_and_is_idempotent() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "Cargo.toml", "[package]\nname=\"demo\"\n");
    common::write(&t.0, "src/main.rs", "fn main() {}\n");
    let run = |args: &[&str]| {
        Command::new(bin()).args(args).output().expect("run claudecat")
    };
    let first = run(&["update", "--root", t.0.to_str().unwrap(), "--top-files", "5"]);
    assert!(first.status.success());
    let content = std::fs::read_to_string(t.0.join("CLAUDE.md")).unwrap();
    assert!(content.contains("<!-- claudecat:auto:begin -->"));
    assert!(content.contains("Project Map"));
    assert!(content.contains("src/main.rs"));
    let second = run(&["update", "--root", t.0.to_str().unwrap(), "--top-files", "5"]);
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(stdout.contains("Up to date"), "expected no rewrite, got: {stdout}");
}
