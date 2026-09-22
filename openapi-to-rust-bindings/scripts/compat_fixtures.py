"""Explicit compatibility fixture profiles; the manifest oracle is not the default envelope."""

from __future__ import annotations

import json
from pathlib import Path
import tomllib


REGISTRY_NAME = "compatibility-cases.json"
IGNORED_PARTS = frozenset({"raw", "target", ".git", "__pycache__"})


def discover_fixtures(root: Path, profile: str) -> list[Path]:
    """Return fixture directories in stable order, never mixing oracle and generic inputs.

    A legacy fixture is opt-in in the checked-in registry AND must contain
    either a checked-in manifest or a generator config requesting one.
    All other OpenAPI fixtures are discoverable as generic, including nested
    scenario directories. Newly added generic fixtures cannot silently enter
    the historical manifest-only compatibility runner.
    """
    if profile not in {"legacy_manifest_oracle", "generic", "all"}:
        raise ValueError(f"unknown compatibility fixture profile: {profile}")
    registry = json.loads((root / REGISTRY_NAME).read_text())
    if (
        registry.get("schema_version") != 1
        or not isinstance(registry.get("legacy_manifest_oracle"), list)
        or any(
            not isinstance(name, str)
            or not name
            or name.startswith("/")
            or ".." in Path(name).parts
            for name in registry["legacy_manifest_oracle"]
        )
    ):
        raise ValueError(f"invalid {REGISTRY_NAME}")
    legacy = registry["legacy_manifest_oracle"]
    if len(set(legacy)) != len(legacy):
        raise ValueError("duplicate legacy fixture in compatibility registry")

    discovered = {
        spec.parent.relative_to(root).as_posix(): spec.parent
        for spec in root.rglob("openapi.json")
        if not any(part in IGNORED_PARTS for part in spec.relative_to(root).parts)
    }
    for name in legacy:
        fixture = discovered.get(name)
        if fixture is None:
            raise ValueError(f"registered legacy fixture is missing openapi.json: {name}")
        manifest = (fixture / "binding-manifest.json").is_file()
        config = fixture / "compat.toml"
        configured = False
        if config.is_file():
            configured = (
                tomllib.loads(config.read_text())
                .get("generator", {})
                .get("binding_manifest")
                is True
            )
        if not (manifest or configured):
            raise ValueError(f"legacy oracle lacks manifest evidence: {name}")

    chosen = {
        "legacy_manifest_oracle": set(legacy),
        "generic": discovered.keys() - set(legacy),
        "all": discovered.keys(),
    }[profile]
    return [discovered[name] for name in sorted(chosen)]
