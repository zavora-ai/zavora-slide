# Changelog

## [Unreleased]

### Security

- Upgrade `zip` from 8.1 to 9.1. `zip` releases before 9.0.5 are affected by
  CVE-2026-115905 (critical).

### Changed

- `zip` 9 made `ZipFile::name` fallible. Opening a package now returns
  `OpcError::Zip` when an entry's name cannot be decoded.

## [0.1.1] - 2026-08-25

### Security

- Upgraded `quick-xml` to 0.41 to close the namespace-allocation and duplicate-
  attribute denial-of-service advisories.
- Preserved entity references in DrawingML text under the parser's hardened
  event model.

## [0.1.0] - 2026-08-14

### Added
- Initial coordinated release of the complete PresentationML engine: faithful
  package I/O, surgical editing, rendering, PDF/Markdown export, charts,
  embedded workbooks, design QA, CLI, and WebAssembly bindings.

### Changed
- Raised the workspace and WASM MSRV to Rust 1.94.1.
- Replaced the obsolete Phase 0 documentation with the implemented capability
  inventory and release architecture.
