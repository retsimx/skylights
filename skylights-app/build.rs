//! Build script for skylights-app.
//!
//! Injects compile-time environment variables:
//! - `SKYLIGHTS_BUILD_VERSION`: Unsigned integer parsed from root `VERSION`
//! - `SKYLIGHTS_GIT_HASH`: Short git commit hash (e.g. `a1b2c3d`) or `"unknown"`
//!
//! Also auto-provisions `src/secrets.rs` from `src/secrets.example.rs` during local development
//! if absent, and emits appropriate `cargo:rerun-if-changed` directives.

use std::path::Path;
use std::process::Command;

/// Auto-provisions `src/secrets.rs` from `src/secrets.example.rs` if absent.
fn ensure_secrets(manifest_path: &Path) {
    let secrets_path = manifest_path.join("src/secrets.rs");
    let secrets_example_path = manifest_path.join("src/secrets.example.rs");
    if !secrets_path.exists() && secrets_example_path.exists() {
        if let Err(err) = std::fs::copy(&secrets_example_path, &secrets_path) {
            println!(
                "cargo:warning=Failed to copy secrets.example.rs to secrets.rs: {}",
                err
            );
        } else {
            println!(
                "cargo:warning=skylights-app/src/secrets.rs was missing; auto-created dummy secrets from secrets.example.rs"
            );
        }
    }
}

/// Reads and parses the root `VERSION` file using `skylights_core::version::parse_version`.
fn read_build_version(manifest_path: &Path) -> u32 {
    let version_path = manifest_path.join("../VERSION");
    let version_raw = std::fs::read_to_string(&version_path).unwrap_or_else(|err| {
        panic!(
            "Failed to read VERSION file at {}: {}",
            version_path.display(),
            err
        )
    });

    skylights_core::version::parse_version(&version_raw).unwrap_or_else(|err| {
        panic!(
            "Failed to parse VERSION file at {}: {}",
            version_path.display(),
            err
        )
    })
}

/// Queries the short git commit hash via `git rev-parse --short HEAD` (fallback to `"unknown"`).
fn query_git_hash(manifest_path: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(manifest_path)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let manifest_path = Path::new(&manifest_dir);

    ensure_secrets(manifest_path);
    let build_version = read_build_version(manifest_path);
    let git_hash = query_git_hash(manifest_path);

    println!("cargo:rustc-env=SKYLIGHTS_BUILD_VERSION={build_version}");
    println!("cargo:rustc-env=SKYLIGHTS_GIT_HASH={git_hash}");
    println!("cargo:rerun-if-changed=../VERSION");
    println!("cargo:rerun-if-changed=src/secrets.rs");
}
