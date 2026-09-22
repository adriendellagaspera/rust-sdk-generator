#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$PWD"
fixture="$repo/tests/fixtures/adopter-stations"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
export RUST_SDK_ADOPTER_CACHE="$work/cache"
crate="$work/field-station-sdk"

cargo build --locked --bin sdk-adopter
/usr/bin/time -p cargo run --locked --bin sdk-adopter -- init \
  --openapi "$fixture/initial.json" --output "$crate" --name field-station-sdk \
  > "$work/init-report.txt" 2> "$work/cold-start.txt"
test -f "$crate/sdkgen.lock.json"
test -f "$crate/.sdkgen/derivation.json"
test -f "$crate/.sdkgen/inventory.json"
test -f "$crate/src/generated/client.rs"
test -f "$crate/src/sdk/mod.rs"
test ! -e "$crate/src/generated/binding-manifest.json"
mkdir -p "$crate/tests"
cp "$fixture/http.rs" "$crate/tests/http.rs"
cargo test --locked --manifest-path "$crate/Cargo.toml" --all-targets

cargo run --locked --bin sdk-adopter -- sync --crate "$crate" --offline --check \
  > "$work/unchanged.json"
python3 - "$work/unchanged.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
assert value["generated_source_diff"] == {"missing":[],"changed":[],"extra":[],"conflicts":[]}, value
assert value["derivation"]["operations"]["read_station"]["status"] == "derived", value
PY

cp "$fixture/updated.json" "$crate/openapi.json"
cargo run --locked --bin sdk-adopter -- sync --crate "$crate" --offline --check \
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
PY
if cargo run --locked --bin sdk-adopter -- sync --crate "$crate" --offline \
    > "$work/rejected-coverage.out" 2> "$work/rejected-coverage.err"; then
  echo 'unexpected coverage drift was accepted without review' >&2; exit 1
fi
grep -q sync.coverage_drift "$work/rejected-coverage.err"
cargo run --locked --bin sdk-adopter -- sync --crate "$crate" \
  --offline --accept-coverage > "$work/sync.txt"
cp "$fixture/http-updated.rs" "$crate/tests/http-updated.rs"
cargo test --locked --manifest-path "$crate/Cargo.toml" --all-targets

find "$crate" -type f ! -path '*/target/*' ! -path '*/.sdkgen/work/*' \
  -print0 | sort -z | xargs -0 sha256sum > "$work/before.sha256"
cargo run --locked --bin sdk-adopter -- sync --crate "$crate" --offline \
  > "$work/idempotent.txt"
find "$crate" -type f ! -path '*/target/*' ! -path '*/.sdkgen/work/*' \
  -print0 | sort -z | xargs -0 sha256sum > "$work/after.sha256"
diff -u "$work/before.sha256" "$work/after.sha256"

# Invalid source, no implicit JSON/YAML/URL fallback.
printf 'not JSON\n' > "$work/invalid.json"
if cargo run --locked --bin sdk-adopter -- init --openapi "$work/invalid.json" \
  --output "$work/invalid-sdk" --name invalid-sdk > /dev/null 2> "$work/invalid.err"; then
  echo 'invalid source accepted' >&2; exit 1
fi
grep -q source.json "$work/invalid.err"
test ! -e "$work/invalid-sdk"

# An offline installation may not silently fetch a different backend.
RUST_SDK_ADOPTER_CACHE="$work/empty-cache" \
  cargo run --locked --bin sdk-adopter -- init --offline \
  --openapi "$fixture/initial.json" --output "$work/offline-sdk" --name offline-sdk \
  > /dev/null 2> "$work/offline.err" && { echo 'incomplete cache accepted' >&2; exit 1; }
grep -q backend.offline_cache "$work/offline.err"

# The exact raw files belong to the generator, not arbitrary files in src/generated.
printf '\n// consumer modification\n' >> "$crate/src/generated/client.rs"
if cargo run --locked --bin sdk-adopter -- sync --crate "$crate" --offline \
  > /dev/null 2> "$work/raw-conflict.err"; then
  echo 'modified raw source was overwritten' >&2; exit 1
fi
grep -q raw.ownership "$work/raw-conflict.err"
sed -i '$d' "$crate/src/generated/client.rs"
sed -i '$d' "$crate/src/generated/client.rs"

# The root publication contract must reject an unmarked consumer-owned facade file.
cp "$crate/src/sdk/stations.rs" "$work/stations.rs"
printf 'handwritten rust\n' > "$crate/src/sdk/stations.rs"
if cargo run --locked --bin sdk-adopter -- sync --crate "$crate" --offline \
  > /dev/null 2> "$work/sdk-conflict.err"; then
  echo 'handwritten facade source was overwritten' >&2; exit 1
fi
grep -q sdk.conflicts "$work/sdk-conflict.err"
cmp -s "$crate/src/sdk/stations.rs" <(printf 'handwritten rust\n')
cp "$work/stations.rs" "$crate/src/sdk/stations.rs"

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
if cargo run --locked --bin sdk-adopter -- sync --crate "$crate" --offline \
  > /dev/null 2> "$work/recipe.err"; then
  echo 'unsupported recipe migration accepted' >&2; exit 1
fi
grep -q recipe.backend_contract "$work/recipe.err"
cp "$work/recipe.json" "$crate/sdkgen.lock.json"

echo 'Independent own-API init/sync, compile, mock HTTP, coverage review, repeatability and fail-closed checks passed'
cat "$work/cold-start.txt"
