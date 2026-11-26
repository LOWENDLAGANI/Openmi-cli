# Building `openmi` on Termux (Android aarch64)

`openmi-cli` is designed to build cleanly on Termux (`aarch64-linux-android`) with **rustls only** — no OpenSSL (`openssl-sys`) or native TLS libraries are required.

## Prerequisites

Inside Termux on an `aarch64` Android device:

```bash
pkg update && pkg upgrade -y
pkg install -y git rust clang pkg-config
```

Verify your toolchain:

```bash
rustc --version
cargo --version
```

## Build & Verify

```bash
git clone https://github.com/LOWENDLAGANI/Openmi-cli.git
cd Openmi-cli

# Confirm no OpenSSL dependency is pulled in
cargo tree -p openmi-cli | grep -i openssl || echo "OK: rustls-only"

# Build the CLI binary
cargo build -p openmi-cli

# Run the built-in smoke test
cargo run -p openmi-cli -- smoke-test

# Run the full workspace test suite
cargo test --workspace
```

For a release binary:

```bash
cargo build -p openmi-cli --release
./target/release/openmi smoke-test
```

## Termux-Specific Notes

- **Temporary files**: Termux uses `$TMPDIR` (`$PREFIX/tmp`) rather than `/tmp`. `std::env::temp_dir()` and `tempfile` automatically honor `$TMPDIR`.
- **Config/Data/Cache paths**: `openmi-platform` resolves XDG paths relative to `$HOME` (`$HOME/.config/openmi`, `$HOME/.local/share/openmi`, `$HOME/.cache/openmi`) unless `XDG_*_HOME` is overridden.
- **TLS / Certificates**: `reqwest` is configured with `default-features = false` and `rustls-tls-webpki-roots`, bundling Mozilla root CAs directly so Android/Termux certificate store quirks do not block HTTPS calls.
- **Process cleanup**: Child commands are placed in their own Unix process group (`process_group(0)`) so cancelling a turn terminates the entire child process tree cleanly.
