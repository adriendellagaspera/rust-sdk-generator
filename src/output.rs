use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::CliError;

#[derive(Debug, Default, Serialize)]
pub(super) struct OutputDiff {
    pub missing: Vec<String>,
    pub changed: Vec<String>,
    pub extra: Vec<String>,
    pub conflicts: Vec<String>,
}

impl OutputDiff {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty()
            && self.changed.is_empty()
            && self.extra.is_empty()
            && self.conflicts.is_empty()
    }
}

fn io_error(path: &Path, action: &str, error: std::io::Error) -> CliError {
    CliError::at("cli.io", path, format!("{action}: {error}"))
}

fn expected_paths(
    files: &BTreeMap<String, String>,
    marker: &str,
) -> Result<BTreeSet<String>, CliError> {
    if marker.is_empty() {
        return Err(CliError::new(
            "cli.output_marker",
            "generated marker must not be empty",
        ));
    }
    let mut paths = BTreeSet::new();
    for (name, source) in files {
        let path = Path::new(name);
        if name.is_empty()
            || !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || !source.starts_with(marker)
        {
            return Err(CliError::new(
                "cli.output_path",
                format!("invalid generated path or missing generated marker: {name:?}"),
            ));
        }
        let canonical = path
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if canonical != *name || !paths.insert(canonical) {
            return Err(CliError::new(
                "cli.output_path",
                format!("non-canonical or duplicate generated path: {name:?}"),
            ));
        }
    }
    for path in &paths {
        let mut parent = Path::new(path).parent();
        while let Some(ancestor) = parent {
            if !ancestor.as_os_str().is_empty() && paths.contains(ancestor.to_str().unwrap_or("")) {
                return Err(CliError::new(
                    "cli.output_path",
                    format!("generated file and directory conflict: {path}"),
                ));
            }
            parent = ancestor.parent();
        }
    }
    Ok(paths)
}

fn validate_root(output: &Path) -> Result<bool, CliError> {
    match fs::symlink_metadata(output) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(CliError::at(
            "cli.output_path",
            output,
            "output must be a directory, not a file or symlink",
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(output, "inspect output", error)),
    }
}

fn walk(
    root: &Path,
    dir: &Path,
    files: &BTreeMap<String, String>,
    marker: &str,
    seen: &mut BTreeSet<String>,
    diff: &mut OutputDiff,
) -> Result<(), CliError> {
    for entry in fs::read_dir(dir).map_err(|error| io_error(dir, "read output directory", error))? {
        let entry = entry.map_err(|error| io_error(dir, "read directory entry", error))?;
        let path = entry.path();
        let ty = entry
            .file_type()
            .map_err(|error| io_error(&path, "inspect entry", error))?;
        if ty.is_symlink() || (!ty.is_dir() && !ty.is_file()) {
            return Err(CliError::at(
                "cli.output_path",
                &path,
                "symlinks and special files in output are not supported",
            ));
        }
        if ty.is_dir() {
            walk(root, &path, files, marker, seen, diff)?;
            continue;
        }
        let relative = path.strip_prefix(root).expect("path under output");
        let Some(name) = relative.to_str() else {
            // Non-UTF-8 handwritten paths are copied but cannot be generated file names.
            continue;
        };
        let source = fs::read(&path).map_err(|error| io_error(&path, "read output file", error))?;
        let owned = source.starts_with(marker.as_bytes());
        if let Some(expected) = files.get(name) {
            seen.insert(name.to_owned());
            if !owned {
                diff.conflicts.push(name.to_owned());
            } else if source != expected.as_bytes() {
                diff.changed.push(name.to_owned());
            }
        } else if owned {
            diff.extra.push(name.to_owned());
        }
    }
    Ok(())
}

pub(super) fn compare(
    output: &Path,
    files: &BTreeMap<String, String>,
    marker: &str,
) -> Result<OutputDiff, CliError> {
    let expected = expected_paths(files, marker)?;
    let mut diff = OutputDiff::default();
    let mut seen = BTreeSet::new();
    if validate_root(output)? {
        walk(output, output, files, marker, &mut seen, &mut diff)?;
    }
    diff.missing = expected.difference(&seen).cloned().collect();
    diff.changed.sort();
    diff.extra.sort();
    diff.conflicts.sort();
    Ok(diff)
}

struct Lock(PathBuf);

impl Lock {
    fn acquire(path: PathBuf) -> Result<Self, CliError> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                let code = if error.kind() == ErrorKind::AlreadyExists {
                    "cli.output_busy"
                } else {
                    "cli.io"
                };
                CliError::at(code, &path, format!("acquire output lock: {error}; remove a stale lock only after checking no generator is running"))
            })?;
        Ok(Self(path))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn sibling_info(output: &Path) -> Result<(PathBuf, String), CliError> {
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CliError::at(
                "cli.output_path",
                output,
                "output must name a directory, not the filesystem root or current directory",
            )
        })?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok((parent.to_path_buf(), name.to_owned()))
}

fn recover_backup(output: &Path, parent: &Path, name: &str) -> Result<(), CliError> {
    if validate_root(output)? {
        return Ok(());
    }
    let prefix = format!(".{name}.rust-sdk-generator-backup-");
    let mut backups = Vec::new();
    for entry in fs::read_dir(parent)
        .map_err(|error| io_error(parent, "scan for interrupted publication", error))?
    {
        let entry = entry.map_err(|error| io_error(parent, "read directory entry", error))?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            backups.push(entry.path());
        }
    }
    match backups.len() {
        0 => Ok(()),
        1 => fs::rename(&backups[0], output)
            .map_err(|error| io_error(output, "restore interrupted output publication", error)),
        _ => Err(CliError::at(
            "cli.output_recovery",
            output,
            format!("multiple output backups found; resolve them before regenerating: {backups:?}"),
        )),
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), CliError> {
    for entry in
        fs::read_dir(source).map_err(|error| io_error(source, "read output directory", error))?
    {
        let entry = entry.map_err(|error| io_error(source, "read directory entry", error))?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let ty = entry
            .file_type()
            .map_err(|error| io_error(&path, "inspect entry", error))?;
        if ty.is_dir() {
            fs::create_dir(&target).map_err(|error| io_error(&target, "stage directory", error))?;
            copy_tree(&path, &target)?;
        } else if ty.is_file() {
            fs::copy(&path, &target)
                .map_err(|error| io_error(&target, "stage handwritten/generated file", error))?;
        } else {
            return Err(CliError::at(
                "cli.output_path",
                &path,
                "symlinks and special files in output are not supported",
            ));
        }
    }
    Ok(())
}

pub(super) fn publish(
    output: &Path,
    files: &BTreeMap<String, String>,
    marker: &str,
) -> Result<(), CliError> {
    expected_paths(files, marker)?;
    let (parent, name) = sibling_info(output)?;
    fs::create_dir_all(&parent)
        .map_err(|error| io_error(&parent, "create output parent", error))?;
    let _lock = Lock::acquire(parent.join(format!(".{name}.rust-sdk-generator.lock")))?;
    recover_backup(output, &parent, &name)?;
    let diff = compare(output, files, marker)?;
    if let Some(conflict) = diff.conflicts.first() {
        return Err(CliError::at(
            "cli.output_conflict",
            &output.join(conflict),
            "refusing to overwrite a file without the current generated marker",
        ));
    }
    if diff.is_clean() && validate_root(output)? {
        return Ok(());
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| CliError::new("cli.io", error.to_string()))?
        .as_nanos();
    let stamp = format!("{}-{nonce}", std::process::id());
    let staged = parent.join(format!(".{name}.rust-sdk-generator-stage-{stamp}"));
    let backup = parent.join(format!(".{name}.rust-sdk-generator-backup-{stamp}"));
    fs::create_dir(&staged)
        .map_err(|error| io_error(&staged, "create staging directory", error))?;
    let _staging = Staging(staged.clone());
    let existed = validate_root(output)?;
    if existed {
        copy_tree(output, &staged)?;
    }
    for name in &diff.extra {
        let path = staged.join(name);
        fs::remove_file(&path)
            .map_err(|error| io_error(&path, "remove stale generated file from stage", error))?;
    }
    for (name, source) in files {
        let path = staged.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| io_error(parent, "stage generated directory", error))?;
        }
        fs::write(&path, source).map_err(|error| io_error(&path, "stage generated file", error))?;
    }
    if existed {
        fs::rename(output, &backup)
            .map_err(|error| io_error(output, "move previous output to backup", error))?;
    }
    if let Err(error) = fs::rename(&staged, output) {
        if existed {
            if let Err(rollback) = fs::rename(&backup, output) {
                return Err(CliError::at(
                    "cli.output_recovery",
                    output,
                    format!(
                        "publication failed ({error}) and rollback failed ({rollback}); previous output is at {}",
                        backup.display()
                    ),
                ));
            }
        }
        return Err(io_error(output, "publish staged output", error));
    }
    if existed {
        if let Err(error) = fs::remove_dir_all(&backup) {
            eprintln!(
                "warning: published output, but could not remove backup {}: {error}",
                backup.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("rust-sdk-output-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn output(&self) -> PathBuf {
            self.0.join("sdk")
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const MARKER: &str = "// @generated test\n";

    fn files(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, body)| ((*name).to_owned(), format!("{MARKER}{body}")))
            .collect()
    }

    #[test]
    fn initial_unchanged_and_read_only_comparison() {
        let temp = Temp::new();
        let output = temp.output();
        let expected = files(&[
            ("mod.rs", "pub mod api;\n"),
            ("api.rs", "pub struct Api;\n"),
        ]);
        assert_eq!(
            compare(&output, &expected, MARKER).unwrap().missing.len(),
            2
        );
        publish(&output, &expected, MARKER).unwrap();
        let before = fs::read(output.join("mod.rs")).unwrap();
        assert!(compare(&output, &expected, MARKER).unwrap().is_clean());
        publish(&output, &expected, MARKER).unwrap();
        assert_eq!(fs::read(output.join("mod.rs")).unwrap(), before);
    }

    #[test]
    fn detects_changed_missing_extra_and_preserves_handwritten_files() {
        let temp = Temp::new();
        let output = temp.output();
        let first = files(&[("mod.rs", "old"), ("old.rs", "stale")]);
        publish(&output, &first, MARKER).unwrap();
        fs::write(output.join("error.rs"), "handwritten runtime\n").unwrap();
        fs::write(output.join("mod.rs"), format!("{MARKER}changed")).unwrap();
        let next = files(&[("mod.rs", "new"), ("nested/new.rs", "new")]);
        let diff = compare(&output, &next, MARKER).unwrap();
        assert_eq!(diff.changed, vec!["mod.rs"]);
        assert_eq!(diff.missing, vec!["nested/new.rs"]);
        assert_eq!(diff.extra, vec!["old.rs"]);
        assert_eq!(
            fs::read_to_string(output.join("mod.rs")).unwrap(),
            format!("{MARKER}changed")
        );
        publish(&output, &next, MARKER).unwrap();
        assert!(compare(&output, &next, MARKER).unwrap().is_clean());
        assert!(!output.join("old.rs").exists());
        assert_eq!(
            fs::read_to_string(output.join("error.rs")).unwrap(),
            "handwritten runtime\n"
        );
    }

    #[test]
    fn refuses_overwriting_a_handwritten_file_and_keeps_old_output() {
        let temp = Temp::new();
        let output = temp.output();
        publish(&output, &files(&[("mod.rs", "old")]), MARKER).unwrap();
        fs::write(output.join("new.rs"), "handwritten").unwrap();
        let requested = files(&[("mod.rs", "new"), ("new.rs", "new")]);
        assert_eq!(
            compare(&output, &requested, MARKER).unwrap().conflicts,
            vec!["new.rs"]
        );
        let error = publish(&output, &requested, MARKER).unwrap_err();
        assert_eq!(error.code, "cli.output_conflict");
        assert_eq!(
            fs::read_to_string(output.join("mod.rs")).unwrap(),
            format!("{MARKER}old")
        );
        assert_eq!(
            fs::read_to_string(output.join("new.rs")).unwrap(),
            "handwritten"
        );
    }

    #[test]
    fn rejects_bad_paths_before_touching_existing_files() {
        let temp = Temp::new();
        let output = temp.output();
        publish(&output, &files(&[("mod.rs", "old")]), MARKER).unwrap();
        for name in [
            "../escape.rs",
            "/absolute.rs",
            "a/./b.rs",
            "mod.rs/child.rs",
        ] {
            let mut bad = files(&[("mod.rs", "new")]);
            bad.insert(name.to_owned(), format!("{MARKER}bad"));
            assert!(publish(&output, &bad, MARKER).is_err(), "{name}");
            assert_eq!(
                fs::read_to_string(output.join("mod.rs")).unwrap(),
                format!("{MARKER}old")
            );
        }
    }

    #[test]
    fn staging_failure_does_not_mutate_previous_output() {
        let temp = Temp::new();
        let output = temp.output();
        publish(&output, &files(&[("mod.rs", "old")]), MARKER).unwrap();
        fs::write(output.join("nested"), "handwritten file").unwrap();
        let error = publish(
            &output,
            &files(&[("mod.rs", "new"), ("nested/new.rs", "new")]),
            MARKER,
        )
        .unwrap_err();
        assert_eq!(error.code, "cli.io");
        assert_eq!(
            fs::read_to_string(output.join("mod.rs")).unwrap(),
            format!("{MARKER}old")
        );
        assert_eq!(
            fs::read_to_string(output.join("nested")).unwrap(),
            "handwritten file"
        );
    }

    #[test]
    fn restores_an_interrupted_backup_and_ignores_old_stage() {
        let temp = Temp::new();
        let output = temp.output();
        let previous = files(&[("mod.rs", "old")]);
        publish(&output, &previous, MARKER).unwrap();
        let backup = temp.0.join(".sdk.rust-sdk-generator-backup-interrupted");
        let abandoned_stage = temp.0.join(".sdk.rust-sdk-generator-stage-interrupted");
        fs::rename(&output, &backup).unwrap();
        fs::create_dir(&abandoned_stage).unwrap();
        fs::write(abandoned_stage.join("partial.rs"), "partial").unwrap();
        // A read-only check must not restore or delete anything.
        assert_eq!(
            compare(&output, &previous, MARKER).unwrap().missing,
            vec!["mod.rs"]
        );
        assert!(backup.exists());
        let next = files(&[("mod.rs", "new")]);
        publish(&output, &next, MARKER).unwrap();
        assert!(compare(&output, &next, MARKER).unwrap().is_clean());
        assert!(!backup.exists());
        assert!(abandoned_stage.join("partial.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_entries_without_mutating_existing_output() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        let output = temp.output();
        publish(&output, &files(&[("mod.rs", "old")]), MARKER).unwrap();
        symlink(output.join("mod.rs"), output.join("alias.rs")).unwrap();
        assert!(publish(&output, &files(&[("mod.rs", "new")]), MARKER).is_err());
        assert_eq!(
            fs::read_to_string(output.join("mod.rs")).unwrap(),
            format!("{MARKER}old")
        );
    }
}
