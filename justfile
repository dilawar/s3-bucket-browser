set dotenv-load := true
set shell := ["bash", "-c"]

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
    @just decrypt
    cargo test

# Full CI gate: fmt + lint + test
ci: fmt-check lint test

# ── Secrets ───────────────────────────────────────────────────────────────────

# Encrypt .env → .env.gpg using GPG_PASSPHRASE
encrypt:
    gpg --batch --yes --passphrase-fd 0 --symmetric --cipher-algo AES256 \
        --output .env.gpg .env <<< "$GPG_PASSPHRASE_GITHUB"

# Decrypt .env.gpg → .env using GPG_PASSPHRASE
decrypt:
    gpg --batch --yes --passphrase-fd 0 --decrypt \
        --output .env .env.gpg <<< "$GPG_PASSPHRASE_GITHUB"

# ── WASM ──────────────────────────────────────────────────────────────────────

# Build WASM package
build-wasm:
    wasm-pack build --target web --out-dir pkg

# Build WASM package and serve locally (requires Python 3)
serve-wasm: build-wasm
    @echo "Open http://localhost:8080 in your browser"
    python3 -m http.server 8080

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
