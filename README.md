# S3 Compatible Bucket Browser

[![Release](https://img.shields.io/github/v/release/dilawar/bucket-browser?style=flat-square)](https://github.com/dilawar/bucket-browser/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/dilawar/bucket-browser/release.yml?style=flat-square&label=build)](https://github.com/dilawar/bucket-browser/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange?style=flat-square)](https://www.rust-lang.org)

A lightweight native GUI for browsing and managing **S3-compatible buckets** —
AWS S3, Backblaze B2, MinIO, and any provider that speaks the S3 API.

![Screenshot]("./images/Screenshot From 2026-04-11 12-12-41.png")

Built with Rust and [egui](https://github.com/emilk/egui).

## Features

- Browse buckets as a file tree
- Upload, download, and delete files and folders
- Multi-select for batch download / delete
- Filter entries by name
- Copy full object paths to clipboard
- Light / dark theme toggle
- Remembers credentials (encrypted on disk)
- Works with any S3-compatible endpoint via a connection URI

## Installation

Download the latest binary for your platform from the [Releases](https://github.com/dilawar/bucket-browser/releases/latest) page.

| Platform       | File                               |
| -------------- | ---------------------------------- |
| Linux x86_64   | `s3-explorer-linux-x86_64.tar.gz`  |
| Windows x86_64 | `s3-explorer-windows-x86_64.zip`   |
| macOS x86_64   | `s3-explorer-macos-x86_64.tar.gz`  |
| macOS ARM64    | `s3-explorer-macos-aarch64.tar.gz` |

## Build from source

```sh
cargo build --release
```

Linux requires a few system libraries for the GUI:

```sh
sudo apt install libxkbcommon-dev libgtk-3-dev libegl1-mesa-dev
```

## Configuration

### Environment variables (fastest)

```sh
export AWS_S3_BUCKET=my-bucket
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
export AWS_DEFAULT_REGION=us-east-1
# For non-AWS providers:
export AWS_ENDPOINT_URL=https://s3.us-west-004.backblazeb2.com
```

### Connection URI

Paste a URI in the connect form or the address bar:

```
s3://my-bucket/
s3://my-bucket/?endpoint=https://s3.us-west-004.backblazeb2.com&region=us-west-004
https://s3.us-west-004.backblazeb2.com/my-bucket
```
