set dotenv-load := true

# Default: list available recipes
default:
    @just --list

# ── Native ────────────────────────────────────────────────────────────────────

# Build (debug)
build:
    cargo build

# Build (release)
build-release:
    cargo build --release

# Run with an optional start directory (e.g. `just run /mnt/s3`)
run dir="/":
    cargo run -- {{ dir }}

# Run (release)
run-release dir="/":
    cargo run --release -- {{ dir }}

# Watch source files and recompile on change (requires cargo-watch)
watch:
    cargo watch -x check

# ── Checks ────────────────────────────────────────────────────────────────────

# Type-check only (fast)
check:
    cargo check

# Lint with Clippy
lint:
    cargo clippy -- -D warnings

fix:
    cargo clippy --fix --allow-dirty


# Format source code
fmt:
    cargo +nightly fmt

# Check formatting without modifying files
fmt-check:
    cargo +nightly fmt -- --check

# Run all tests
test:
    cargo test

# Full CI gate: fmt + lint + test
ci: fmt-check lint test

# ── Maintenance ───────────────────────────────────────────────────────────────

# Update dependencies
update:
    cargo update

# Check for outdated dependencies (requires cargo-outdated)
outdated:
    cargo outdated

# Remove build artefacts
clean:
    cargo clean
    rm -rf pkg/

# ── Packaging ─────────────────────────────────────────────────────────────────

# Generate icons from SVG source (requires ImageMagick)
generate-icons:
    ./assets/generate-icons.sh

# Build Debian package (.deb)
package-deb:
    cargo deb

# Build RPM package (.rpm)
package-rpm:
    cargo generate-rpm

# Build all packages
package-all: package-deb package-rpm
