//! The only implemented raw-backend driver. No raw layout or backend CLI leaks into
//! the orchestration's canonical Bindings -> derive/generate boundary.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{Result, err, run, write_if_changed};

pub const BACKEND_ID: &str = "openapi-to-rust/upstream-v1";
pub const REPOSITORY: &str = "gpu-cli/openapi-to-rust";
pub const ADAPTER_ID: &str = "openapi-to-rust-bindings/manifest-free-v3";
pub const TEMPLATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackendLock {
    pub id: String,
    pub repository: String,
    pub revision: String,
    pub adapter: String,
    pub bindings_version: u32,
    pub config_version: u32,
    pub generated_layout: Vec<String>,
    pub module_name: String,
    pub base_url: String,
    pub retry_max: u32,
    pub prune_models: bool,
}

pub struct DriverOutput {
    pub raw: PathBuf,
    pub dependencies: String,
    pub rust: BTreeMap<String, Vec<u8>>,
    pub bindings: Vec<u8>,
}

pub trait BackendDriver {
    fn generate(
        &self,
        lock: &BackendLock,
        work: &Path,
        cache: &Path,
        offline: bool,
        adapter: &Path,
    ) -> Result<DriverOutput>;
}

pub struct UpstreamDriver;

pub fn default_lock(pin: &str) -> BackendLock {
    BackendLock {
        id: BACKEND_ID.into(),
        repository: REPOSITORY.into(),
        revision: pin.into(),
        adapter: ADAPTER_ID.into(),
        bindings_version: 3,
        config_version: 1,
        generated_layout: vec!["client.rs".into(), "types.rs".into(), "mod.rs".into(), "REQUIRED_DEPS.toml".into()],
        module_name: "sdk".into(),
        base_url: "http://127.0.0.1".into(),
        retry_max: 0,
        prune_models: false,
    }
}

pub fn verify_lock(lock: &BackendLock, default_pin: &str) -> Result<()> {
    // A different backend requires an explicit driver and recipe migration; no
    // silent upgrade or speculative implementation of a second producer.
    if lock != &default_lock(default_pin) {
        return Err(err("recipe.backend_contract", "unsupported/migrated backend recipe; the pinned upstream driver requires exactly the recorded v1 layout, adapter v3, raw options and immutable default revision"));
    }
    Ok(())
}

fn checkout(lock: &BackendLock, cache: &Path, offline: bool) -> Result<PathBuf> {
    let directory = cache.join("backends").join(&lock.revision);
    if !directory.join(".git").is_dir() {
        if offline {
            return Err(err("backend.offline_cache", format!("missing pinned checkout {}; bootstrap once without --offline", directory.display())));
        }
        fs::create_dir_all(&directory).map_err(|e| err("backend.checkout", e))?;
        run("backend.checkout", Command::new("git").arg("init").arg(&directory))?;
        run("backend.checkout", Command::new("git").args(["-C"]).arg(&directory).args(["remote", "add", "origin", "https://github.com/gpu-cli/openapi-to-rust.git"]))?;
        run("backend.checkout", Command::new("git").args(["-C"]).arg(&directory).args(["fetch", "--depth=1", "origin"]).arg(&lock.revision))?;
        run("backend.checkout", Command::new("git").args(["-C"]).arg(&directory).args(["checkout", "--detach", "FETCH_HEAD"]))?;
    }
    let head = run("backend.pin", Command::new("git").arg("-C").arg(&directory).args(["rev-parse", "HEAD"]))?;
    if String::from_utf8_lossy(&head.stdout).trim() != lock.revision {
        return Err(err("backend.pin", format!("cached source {} is not the locked revision {}; do not use a mutable checkout", directory.display(), lock.revision)));
    }
    let status = run("backend.pin", Command::new("git").arg("-C").arg(&directory).args(["status", "--porcelain"]))?;
    if !status.stdout.is_empty() {
        return Err(err("backend.pin", format!("cached backend source has uncommitted modifications: {}", directory.display())));
    }
    Ok(directory)
}

fn binary(lock: &BackendLock, cache: &Path, offline: bool) -> Result<PathBuf> {
    let checkout = checkout(lock, cache, offline)?;
    let target = cache.join("backend-target").join(&lock.revision);
    let executable = target.join("release").join(format!("openapi-to-rust{}", std::env::consts::EXE_SUFFIX));
    if !executable.is_file() {
        if offline {
            return Err(err("backend.offline_cache", format!("missing compiled backend {}; bootstrap once without --offline", executable.display())));
        }
        let mut build = Command::new("cargo");
        build.args(["build", "--locked", "--release", "--manifest-path"])
            .arg(checkout.join("Cargo.toml")).args(["--bin", "openapi-to-rust"])
            .env("CARGO_TARGET_DIR", &target);
        run("backend.build", &mut build)?;
    }
    Ok(executable)
}

impl BackendDriver for UpstreamDriver {
    fn generate(
        &self,
        lock: &BackendLock,
        work: &Path,
        cache: &Path,
        offline: bool,
        adapter: &Path,
    ) -> Result<DriverOutput> {
        fs::create_dir_all(work.join("raw")).map_err(|e| err("raw.output", e))?;
        let config = format!(
            "[generator]\nspec_path = \"effective-openapi.json\"\noutput_dir = \"raw\"\nmodule_name = \"{}\"\n\n[features]\nenable_async_client = true\n\n[http_client]\nbase_url = \"{}\"\n\n[http_client.retry]\nmax_retries = {}\n\n[client]\nprune_models = {}\n",
            lock.module_name, lock.base_url, lock.retry_max, lock.prune_models
        );
        write_if_changed(&work.join("openapi-to-rust.toml"), config.as_bytes(), "raw.config")?;
        run("raw.generate", Command::new(binary(lock, cache, offline)?)
            .args(["generate", "--config", "openapi-to-rust.toml"])
            .current_dir(work))?;
        let raw = work.join("raw");
        for name in &lock.generated_layout {
            if !raw.join(name).is_file() {
                return Err(err("raw.layout", format!("pinned backend did not produce {}", raw.join(name).display())));
            }
        }
        if raw.join("binding-manifest.json").exists() {
            return Err(err("raw.manifest", "the selected upstream driver must not emit or consume a fork-only binding manifest"));
        }
        let mut rust = BTreeMap::new();
        for entry in fs::read_dir(&raw).map_err(|e| err("raw.layout", e))? {
            let entry = entry.map_err(|e| err("raw.layout", e))?;
            let path = entry.path();
            if path.extension().is_some_and(|value| value == "rs") {
                let name = entry.file_name().into_string().map_err(|_| err("raw.layout", "non-UTF-8 generated source filename"))?;
                if !["client.rs", "types.rs", "mod.rs"].contains(&name.as_str()) {
                    return Err(err("raw.layout", format!("unreviewed raw Rust layout: {name}")));
                }
                rust.insert(name, fs::read(&path).map_err(|e| err("raw.layout", e))?);
            }
        }
        let dependencies = fs::read_to_string(raw.join("REQUIRED_DEPS.toml")).map_err(|e| err("raw.dependencies", e))?;
        if !dependencies.lines().any(|line| line.trim() == "[dependencies]") {
            return Err(err("raw.dependencies", "pinned backend did not provide a [dependencies] fragment"));
        }
        let result = run("adapter.extract", Command::new(adapter).arg(&raw).arg(work.join("effective-openapi.json")))?;
        Ok(DriverOutput { raw, dependencies, rust, bindings: result.stdout })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_backend_or_contract_migration() {
        let mut lock = default_lock("0123456789012345678901234567890123456789");
        verify_lock(&lock, &lock.revision).unwrap();
        lock.adapter = "openapi-generator/other".into();
        assert!(verify_lock(&lock, "0123456789012345678901234567890123456789").is_err());
    }
}
