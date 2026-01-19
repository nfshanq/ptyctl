use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    let git_sha = resolve_git_sha();
    let build_time = resolve_build_time();

    println!("cargo:rustc-env=PTYCTL_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=PTYCTL_BUILD_TIME={build_time}");

    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
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
