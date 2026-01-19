use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let git_sha = resolve_git_sha();
    let git_tag = resolve_git_tag();
    let version_label = resolve_version_label(&git_tag);
    let build_time = resolve_build_time();
    let build_stamp = resolve_build_stamp_path();

    println!("cargo:rustc-env=PTYCTL_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=PTYCTL_GIT_TAG={git_tag}");
    println!("cargo:rustc-env=PTYCTL_VERSION_LABEL={version_label}");
    println!("cargo:rustc-env=PTYCTL_BUILD_TIME={build_time}");

    println!("cargo:rerun-if-changed=build/build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    println!("cargo:rerun-if-changed=.git/refs/tags");
    if let Some(path) = &build_stamp {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    write_build_stamp(build_stamp, &build_time);
}

fn resolve_git_sha() -> String {
    if let Ok(value) = env::var("GITHUB_SHA") {
        return normalize_git_sha(value);
    }

    if !Path::new(".git").exists() {
        return String::new();
    }

    let mut sha = match git_output(&["rev-parse", "--short", "HEAD"]) {
        Some(value) => value,
        None => return String::new(),
    };
    let dirty = env::var("PTYCTL_GIT_DIRTY")
        .ok()
        .and_then(|value| parse_bool(&value))
        .unwrap_or_else(|| {
            git_output(&["status", "--porcelain"]).is_some_and(|s| !s.trim().is_empty())
        });
    if dirty {
        sha = format!("{sha}-dirty");
    }
    sha
}

fn resolve_git_tag() -> String {
    if let Ok(value) = env::var("PTYCTL_GIT_TAG") {
        if !value.trim().is_empty() {
            return value.trim().to_string();
        }
    }

    if !Path::new(".git").exists() {
        return String::new();
    }

    if let Some(tag) = git_output(&["describe", "--tags", "--abbrev=0"]) {
        return tag;
    }

    if let Some(tags) = git_output(&["tag", "--sort=-creatordate"]) {
        if let Some(first) = tags.lines().next() {
            if !first.trim().is_empty() {
                return first.trim().to_string();
            }
        }
    }

    String::new()
}

fn resolve_version_label(git_tag: &str) -> String {
    if let Ok(value) = env::var("PTYCTL_VERSION_LABEL") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let trimmed_tag = git_tag.trim();
    if !trimmed_tag.is_empty() {
        return trimmed_tag.to_string();
    }

    env::var("CARGO_PKG_VERSION").unwrap_or_default()
}

fn normalize_git_sha(value: String) -> String {
    let trimmed = value.trim();
    if trimmed.len() > 7 {
        trimmed[..7].to_string()
    } else if trimmed.is_empty() {
        String::new()
    } else {
        trimmed.to_string()
    }
}

fn resolve_build_time() -> String {
    if let Ok(value) = env::var("PTYCTL_BUILD_TIME") {
        if !value.trim().is_empty() {
            return value;
        }
    }
    if let Ok(value) = env::var("SOURCE_DATE_EPOCH") {
        if let Ok(epoch) = value.trim().parse::<i64>() {
            if let Ok(dt) = time::OffsetDateTime::from_unix_timestamp(epoch) {
                if let Ok(text) = dt.format(&time::format_description::well_known::Rfc3339) {
                    return text;
                }
            }
        }
    }

    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

fn resolve_build_stamp_path() -> Option<PathBuf> {
    env::var("OUT_DIR")
        .ok()
        .map(|out_dir| PathBuf::from(out_dir).join("build-time.stamp"))
}

fn write_build_stamp(path: Option<PathBuf>, build_time: &str) {
    let Some(path) = path else {
        println!("cargo:warning=PTYCTL build stamp disabled: OUT_DIR not set");
        return;
    };

    if let Err(err) = fs::write(&path, build_time) {
        println!(
            "cargo:warning=PTYCTL failed to write build stamp {}: {}",
            path.display(),
            err
        );
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let value = text.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}
