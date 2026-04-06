#!/usr/bin/env bash
# Generate Fumadocs MDX content from Rust doc comments and cargo doc HTML.
#
# Run from the repo root:
#   bash docs/scripts/generate.sh
#
# This script:
#   1. Extracts //! crate-level doc comments from each crate's lib.rs
#   2. Generates MDX files in docs/content/docs/
#   3. Builds cargo doc HTML and copies it to docs/public/api/

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CONTENT="$ROOT/docs/content/docs"

rm -rf "$CONTENT"
mkdir -p "$CONTENT"

# ---------------------------------------------------------------------------
# Generate crate pages from //! doc comments
# ---------------------------------------------------------------------------

generate_crate_page() {
    local crate_dir="$1"
    local crate_name
    crate_name="$(basename "$crate_dir")"

    local src_file="$crate_dir/src/lib.rs"
    if [[ "$crate_name" == "sonic-mgmt-cli" ]]; then
        src_file="$crate_dir/src/main.rs"
    fi
    [[ -f "$src_file" ]] || return 0

    # Extract //! lines, strip prefix
    local docs
    docs="$(sed -n 's|^//!\( \?\)||p' "$src_file")"
    [[ -n "$docs" ]] || return 0

    # First non-empty line becomes the description
    local desc
    desc="$(echo "$docs" | sed '/^$/d' | head -1 | sed 's/[`]//g')"

    # Pretty title: sonic-core -> sonic_core (matches rustdoc)
    local rustdoc_name="${crate_name//-/_}"

    local out="$CONTENT/$crate_name.mdx"
    cat > "$out" <<EOF
---
title: "$crate_name"
description: "$desc"
---

$docs

## API Reference

Full type and trait documentation is available in the
[rustdoc output](/api/$rustdoc_name/index.html).
EOF
    echo "  $crate_name"
}

echo "Generating crate pages..."
for crate_dir in "$ROOT"/crates/*/; do
    generate_crate_page "$crate_dir"
done

# ---------------------------------------------------------------------------
# Generate index page
# ---------------------------------------------------------------------------

cat > "$CONTENT/index.mdx" <<'INDEXEOF'
---
title: sonic-mgmt-rs
description: Rust-based network management framework for SONiC switches
---

## Crates

| Crate | Description |
|---|---|
| [sonic-core](/docs/sonic-core) | Core types, traits, and error definitions |
| [sonic-config](/docs/sonic-config) | TOML/YAML configuration loading and validation |
| [sonic-device](/docs/sonic-device) | Device abstraction layer with SSH, Telnet, and console transport |
| [sonic-testbed](/docs/sonic-testbed) | Testbed management, provisioning, and lifecycle |
| [sonic-topology](/docs/sonic-topology) | Network topology generation and rendering |
| [sonic-testing](/docs/sonic-testing) | Test discovery, execution, and result collection |
| [sonic-reporting](/docs/sonic-reporting) | JUnit XML parsing, test analytics, and coverage reporting |
| [sonic-sdn](/docs/sonic-sdn) | SDN protocol clients for gNMI, gNOI, and P4Runtime |
| [sonic-mgmt-cli](/docs/sonic-mgmt-cli) | CLI binary tying everything together |
| [sonic-integration-tests](/docs/sonic-integration-tests) | Integration tests against live SONiC devices |

## API Reference

Browse the full [rustdoc API reference](/api/sonic_core/index.html).
INDEXEOF

# ---------------------------------------------------------------------------
# Generate meta.json for sidebar navigation
# ---------------------------------------------------------------------------

cat > "$CONTENT/meta.json" <<'METAEOF'
{
  "title": "Documentation",
  "pages": [
    "index",
    "---Crates---",
    "sonic-core",
    "sonic-config",
    "sonic-device",
    "sonic-testbed",
    "sonic-topology",
    "sonic-testing",
    "sonic-reporting",
    "sonic-sdn",
    "sonic-mgmt-cli",
    "sonic-integration-tests"
  ]
}
METAEOF

# ---------------------------------------------------------------------------
# Build cargo doc and copy to public/api/
# ---------------------------------------------------------------------------

echo "Building cargo doc..."
cargo doc --workspace --no-deps --target-dir "$ROOT/target" 2>/dev/null
rm -rf "$ROOT/docs/public/api"
cp -r "$ROOT/target/doc" "$ROOT/docs/public/api"
echo "Done. Output in docs/content/docs/ and docs/public/api/"
