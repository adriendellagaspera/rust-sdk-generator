#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$PWD"
fixture="$repo/tests/fixtures/own-api-stations"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
export RUST_SDK_CACHE="$work/cache"
crate="$work/field-station-sdk"

# Time the *full* clean-runner Cargo init, including compiling the rust-sdk CLI.
/usr/bin/time -p cargo run --locked --bin rust-sdk -- init \
  --openapi "$fixture/initial.json" --output "$crate" --name field-station-sdk \
  > "$work/init-report.txt" 2> "$work/cold-start.txt"
test -f "$crate/sdkgen.lock.json"
test -f "$crate/.sdkgen/derivation.json"
test -f "$crate/.sdkgen/inventory.json"
test -f "$crate/src/generated/client.rs"
test -f "$crate/src/sdk/mod.rs"
test ! -e "$crate/src/generated/binding-manifest.json"
# Architectural boundary: the actual pinned driver produces canonical Bindings
# v3 with preserved source operation identity; root derive/generate consumes only
# this data, without any backend/adapter dependency in the standalone crate.
python3 - "$crate" <<'PY'
import json, pathlib, sys
crate = pathlib.Path(sys.argv[1])
bindings = json.loads((crate / ".sdkgen/work/rust-bindings.json").read_text())
definition = json.loads((crate / ".sdkgen/work/definition.json").read_text())
inventory = json.loads((crate / ".sdkgen/inventory.json").read_text())
assert bindings["schema_version"] == 3, bindings
source = {entry["metadata"]["source_operation"]["operation_id"]
          for entry in bindings["operations"].values()}
assert source == {"read_station"}, source
assert definition["schema_version"] == 2, definition
assert inventory["resources"], inventory
manifest = (crate / "Cargo.toml").read_text()
assert "rust-sdk-generator" not in manifest
assert "openapi-to-rust-bindings" not in manifest
PY
mkdir -p "$crate/tests"
cp "$fixture/http.rs" "$crate/tests/http.rs"
cargo test --locked --manifest-path "$crate/Cargo.toml" --all-targets

cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline --check \
  > "$work/unchanged.json"
python3 - "$work/unchanged.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
assert value["generated_source_diff"] == {"missing":[],"changed":[],"extra":[],"conflicts":[]}, value
assert value["derivation"]["operations"]["read_station"]["status"] == "derived", value
assert value["public_api_inventory"]["resources"], value
assert value["generated_source_changes"] == [], value
PY

cp "$fixture/updated.json" "$crate/openapi.json"
cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline --check \
  > "$work/changed.json"
python3 - "$work/changed.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
operations = value["derivation"]["operations"]
assert set(operations) == {"read_station", "delete_station"}, operations
assert all(item["status"] == "derived" for item in operations.values()), operations
diff = value["generated_source_diff"]
assert diff["missing"] or diff["changed"], diff
assert not diff["conflicts"], diff
changes = value["generated_source_changes"]
assert changes, value
assert any(entry["previous"] != entry["candidate"] for entry in changes), changes
assert value["public_api_inventory"]["resources"], value
PY
if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline \
    > "$work/rejected-coverage.out" 2> "$work/rejected-coverage.err"; then
  echo 'unexpected coverage drift was accepted without review' >&2; exit 1
fi
grep -q sync.coverage_drift "$work/rejected-coverage.err"
cargo run --locked --bin rust-sdk -- sync --crate "$crate" \
  --offline --accept-coverage > "$work/sync.txt"
cp "$fixture/http-updated.rs" "$crate/tests/http-updated.rs"
cargo test --locked --manifest-path "$crate/Cargo.toml" --all-targets

find "$crate" -type f ! -path '*/target/*' ! -path '*/.sdkgen/work/*' \
  -print0 | sort -z | xargs -0 sha256sum > "$work/before.sha256"
cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline \
  > "$work/idempotent.txt"
find "$crate" -type f ! -path '*/target/*' ! -path '*/.sdkgen/work/*' \
  -print0 | sort -z | xargs -0 sha256sum > "$work/after.sha256"
diff -u "$work/before.sha256" "$work/after.sha256"

# Unchanged source/recipe must never silently republish even a still-marked
# generated facade. Preserve the byte-level difference for explicit review.
cp "$crate/src/sdk/stations.rs" "$work/accepted-stations.rs"
printf '\n// unexpected still-marked generated drift\n' >> "$crate/src/sdk/stations.rs"
if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline \
    > "$work/nondeterminism.out" 2> "$work/nondeterminism.err"; then
  echo 'unmodified-input generated source drift was silently replaced' >&2; exit 1
fi
grep -q sync.nondeterminism "$work/nondeterminism.err"
grep -q generated_source_changes "$work/nondeterminism.out"
grep -q 'unexpected still-marked generated drift' "$crate/src/sdk/stations.rs"
cp "$work/accepted-stations.rs" "$crate/src/sdk/stations.rs"

# Invalid source, no implicit JSON/YAML/URL fallback.
printf 'not JSON\n' > "$work/invalid.json"
if cargo run --locked --bin rust-sdk -- init --openapi "$work/invalid.json" \
  --output "$work/invalid-sdk" --name invalid-sdk > /dev/null 2> "$work/invalid.err"; then
  echo 'invalid source accepted' >&2; exit 1
fi
grep -q source.json "$work/invalid.err"
test ! -e "$work/invalid-sdk"

# An unsupported ordinary upstream stream may not become an omitted operation.
if cargo run --locked --bin rust-sdk -- init   --openapi "$repo/openapi-to-rust-bindings/tests/fixtures/capability-v1/sse/openapi.json"   --output "$work/unsupported-sdk" --name unsupported-sdk   > "$work/unsupported.out" 2> "$work/unsupported.err"; then
  echo 'unproven SSE operation accepted as an SDK' >&2; exit 1
fi
grep -Eq '(raw.generate|adapter.extract|derive.unsupported|derive.contract|bindings.v3)' "$work/unsupported.err"
test ! -e "$work/unsupported-sdk"

# No producer manifest or fabricated Bindings from absent ordinary Rust.
mkdir -p "$work/empty-raw"
if cargo run --locked -p openapi-to-rust-bindings -- "$work/empty-raw" "$fixture/initial.json"   > /dev/null 2> "$work/missing-evidence.err"; then
  echo 'missing backend evidence was accepted' >&2; exit 1
fi
grep -q adapter.extract "$work/missing-evidence.err"

# An offline installation may not silently fetch a different backend.
RUST_SDK_CACHE="$work/empty-cache" \
  cargo run --locked --bin rust-sdk -- init --offline \
  --openapi "$fixture/initial.json" --output "$work/offline-sdk" --name offline-sdk \
  > /dev/null 2> "$work/offline.err" && { echo 'incomplete cache accepted' >&2; exit 1; }
grep -q backend.offline_cache "$work/offline.err"

# The exact raw files belong to the generator, not arbitrary files in src/generated.
printf '\n// consumer modification\n' >> "$crate/src/generated/client.rs"
if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline \
  > /dev/null 2> "$work/raw-conflict.err"; then
  echo 'modified raw source was overwritten' >&2; exit 1
fi
grep -q raw.ownership "$work/raw-conflict.err"
sed -i '$d' "$crate/src/generated/client.rs"
sed -i '$d' "$crate/src/generated/client.rs"

# The root publication contract must reject an unmarked consumer-owned facade file.
cp "$crate/src/sdk/stations.rs" "$work/stations.rs"
printf 'handwritten rust\n' > "$crate/src/sdk/stations.rs"
if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline \
  > /dev/null 2> "$work/sdk-conflict.err"; then
  echo 'handwritten facade source was overwritten' >&2; exit 1
fi
grep -q sdk.conflicts "$work/sdk-conflict.err"
cmp -s "$crate/src/sdk/stations.rs" <(printf 'handwritten rust\n')
cp "$work/stations.rs" "$crate/src/sdk/stations.rs"

# Missing or changed accepted reports must fail closed before touching source.
for report in derivation inventory; do
  cp "$crate/.sdkgen/$report.json" "$work/accepted-$report.json"
  rm "$crate/.sdkgen/$report.json"
  if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline     > /dev/null 2> "$work/missing-$report.err"; then
    echo "missing $report audit evidence was accepted" >&2; exit 1
  fi
  grep -q recipe.audit_drift "$work/missing-$report.err"
  cp "$work/accepted-$report.json" "$crate/.sdkgen/$report.json"
  printf '\n' >> "$crate/.sdkgen/$report.json"
  if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline     > /dev/null 2> "$work/stale-$report.err"; then
    echo "stale $report audit evidence was accepted" >&2; exit 1
  fi
  grep -q recipe.audit_drift "$work/stale-$report.err"
  cp "$work/accepted-$report.json" "$crate/.sdkgen/$report.json"
done

# Recipe backend/adapter migrations are explicit and cannot be inferred.
cp "$crate/sdkgen.lock.json" "$work/recipe.json"
python3 - "$crate/sdkgen.lock.json" <<'PY'
import json, sys
path = sys.argv[1]
recipe = json.load(open(path))
recipe["backend"]["adapter"] = "alternate-adapter"
with open(path, "w") as output:
    json.dump(recipe, output)
PY
if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline \
  > /dev/null 2> "$work/recipe.err"; then
  echo 'unsupported recipe migration accepted' >&2; exit 1
fi
grep -q recipe.backend_contract "$work/recipe.err"
cp "$work/recipe.json" "$crate/sdkgen.lock.json"

# A changed backend revision is not a permitted implicit sync migration.
python3 - "$crate/sdkgen.lock.json" <<'PY'
import json, sys
path = sys.argv[1]
recipe = json.load(open(path))
recipe["backend"]["revision"] = "0" * 40
with open(path, "w") as output:
    json.dump(recipe, output)
PY
if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline   > /dev/null 2> "$work/changed-revision.err"; then
  echo 'changed backend revision was silently accepted' >&2; exit 1
fi
grep -q recipe.backend_contract "$work/changed-revision.err"
cp "$work/recipe.json" "$crate/sdkgen.lock.json"

# A checkout without its compiled binary is an incomplete offline cache, not a
# reason to fetch/build over the network despite the offline request.
revision="$(python3 - "$crate/sdkgen.lock.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["backend"]["revision"])
PY
)"
backend_bin="$RUST_SDK_CACHE/backend-target/$revision/release/openapi-to-rust"
test -f "$backend_bin"
mv "$backend_bin" "$work/backend-bin"
if cargo run --locked --bin rust-sdk -- sync --crate "$crate" --offline   > /dev/null 2> "$work/incomplete-cache.err"; then
  echo 'incomplete compiled offline cache was accepted' >&2; exit 1
fi
grep -q backend.offline_cache "$work/incomplete-cache.err"
mv "$work/backend-bin" "$backend_bin"

echo 'Independent own-API init/sync, compile, mock HTTP, coverage review, repeatability and fail-closed checks passed'
cat "$work/cold-start.txt"
