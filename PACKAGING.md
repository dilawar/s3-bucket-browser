# Packaging Guide

This document explains how to build Debian (.deb) and RPM (.rpm) packages for s3-explorer.

## Prerequisites

You need to install two Cargo extension tools:

```bash
# For Debian/Ubuntu packages
cargo install cargo-deb

# For RPM packages (Fedora, RHEL, CentOS, openSUSE, etc.)
cargo install cargo-generate-rpm
```

## Building Packages

### Using Just (Recommended)

```bash
# Build Debian package (.deb)
just package-deb

# Build RPM package (.rpm)
just package-rpm

# Build both packages
just package-all
```

### Using Cargo Directly

```bash
# Build Debian package
cargo deb

# Build RPM package
cargo generate-rpm
```

## Output Location

After building, you'll find the packages in:

- **Debian package**: `target/debian/s3-explorer_*.deb`
- **RPM package**: `target/generate-rpm/s3-explorer-*.rpm`

## Installing the Packages

### Debian/Ubuntu (.deb)

```bash
sudo dpkg -i target/debian/s3-explorer_*.deb
```

Or with apt to handle dependencies:

```bash
sudo apt install ./target/debian/s3-explorer_*.deb
```

### Fedora/RHEL/CentOS/openSUSE (.rpm)

```bash
# Fedora/RHEL/CentOS
sudo dnf install ./target/generate-rpm/s3-explorer-*.rpm

# openSUSE
sudo zypper install ./target/generate-rpm/s3-explorer-*.rpm
```

## Package Configuration

Package metadata is configured in `Cargo.toml`:

### Desktop Integration

The packages include proper desktop integration:

- **Desktop entry file**: `s3-explorer.desktop` for application menu integration
- **Icons**: Multiple sizes (16x16 to 512x512 PNG + SVG) following the freedesktop.org icon theme specification
- **Installation path**: Icons are installed to `/usr/share/icons/hicolor/` for automatic theme support

### Debian Package (`[package.metadata.deb]`)

- **maintainer**: Package maintainer information
- **copyright**: Copyright information
- **license-file**: License file path and permissions
- **extended-description**: Package description
- **section**: Debian package section
- **priority**: Package priority
- **assets**: Files to include in the package
- **depends**: Package dependencies (`$auto` for automatic dependency detection)

### RPM Package (`[package.metadata.generate-rpm]`)

- **assets**: Array of files to include with source path, destination path, mode, and doc flag

## Customizing Packages

You can customize the package contents by modifying the `[package.metadata.deb]` and `[package.metadata.generate-rpm]` sections in `Cargo.toml`.

### Example: Adding Desktop Entry

To add a `.desktop` file for GUI applications:

1. Create `assets/s3-explorer.desktop`:
```ini
[Desktop Entry]
Name=S3 Explorer
Comment=A native GUI for browsing S3-compatible buckets
Exec=s3-explorer
Icon=s3-explorer
Terminal=false
Type=Application
Categories=Utility;
```

2. Update the assets array in `Cargo.toml`:

```toml
[package.metadata.deb]
assets = [
    ["target/release/s3-explorer", "usr/bin/", "755"],
    ["README.md", "usr/share/doc/s3-explorer/README", "644"],
    ["assets/s3-explorer.desktop", "usr/share/applications/", "644"],
    ["assets/icon.png", "usr/share/icons/hicolor/256x256/apps/s3-explorer.png", "644"],
]
```

## Troubleshooting

### Missing Dependencies

If `cargo-deb` or `cargo-generate-rpm` are not found:

```bash
cargo install cargo-deb
cargo install cargo-generate-rpm
```

### Build Errors

Make sure you build the release binary first:

```bash
cargo build --release
```

Then run the packaging command.

### Custom Output Location

To specify a custom output path:

```bash
# Debian
cargo deb --output ./pkg/s3-explorer.deb

# RPM
cargo generate-rpm --output-pkg-path ./pkg/s3-explorer.rpm
```
