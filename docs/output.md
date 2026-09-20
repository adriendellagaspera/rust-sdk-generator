# Generated output and publication

Use `generate --output DIR` with a **dedicated SDK directory**, not the repository root. Library `generate()` returns an in-memory generated-file map and inventory; the CLI alone publishes them.

A file is generator-owned only if its bytes begin with the exact `Runtime.generated_marker` for the current invocation. The CLI refuses to overwrite an unmarked file at an expected generated path, preserves unrelated handwritten files, and removes obsolete marked generated files. Changing the marker is a migration: files with the former marker become unmanaged and will conflict rather than be deleted. The existing default marker is retained for compatibility even if its wording differs from the current crate name.

Generated relative paths must be canonical and remain within the output directory. The CLI rejects symlinks and special files inside that directory rather than following them. It stages a copy of the complete output tree and writes the new generated files into it before publication. Generation for the same destination is serialized by `.<output-name>.rust-sdk-generator.lock` in the parent directory.

Publication renames the old directory to a uniquely named sibling backup, then renames the staged directory into place. A normal second-rename error triggers an attempted rollback. This is **not** a filesystem-wide atomic transaction: external readers may see the interval between the two renames. After an operating-system crash in that interval, the next `generate` restores a single detected backup; multiple backups require manual resolution. `check-generated` never attempts recovery or writes the filesystem.

Remove an abandoned lock only after confirming that no generator is running. Abandoned staging directories are ignored and may be cleaned up manually. An optional `--inventory` path outside the generated directory is written separately after publishing, so its update is not part of the directory swap. Coordinate reader access with generation if uninterrupted visibility is required.

`check-generated` compares the expected map with on-disk files and reports `missing`, `changed`, `extra` and `conflicts`; a file at an expected path without the marker is a conflict, not an overwrite candidate. A clean comparison exits 0, differences exit 1, and IO/input errors exit 2.

Do not use a mutable output directory that another process edits concurrently: the lock coordinates participating generator processes, not arbitrary writers or readers.
