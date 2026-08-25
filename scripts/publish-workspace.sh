#!/usr/bin/env bash
set -euo pipefail

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
packages=(
  zavora-slide-opc
  zavora-slide-oxml
  zavora-slide-layout
  zavora-slide-render
  zavora-slide-xlsx
  zavora-slide-pdf
  zavora-slide
  zavora-slide-cli
)

for package in "${packages[@]}"; do
  if curl --fail --silent --show-error \
    --user-agent "zavora-slide-release/${version}" \
    "https://crates.io/api/v1/crates/${package}/${version}" >/dev/null 2>&1; then
    echo "${package} ${version} is already published"
    continue
  fi
  cargo publish --locked -p "${package}"
done

wasm_manifest=crates/zavora-slide-wasm/Cargo.toml
if ! curl --fail --silent --show-error \
  --user-agent "zavora-slide-release/${version}" \
  "https://crates.io/api/v1/crates/zavora-slide-wasm/${version}" >/dev/null 2>&1; then
  cargo publish --locked --manifest-path "$wasm_manifest"
fi
