# UPKG - Universal Package Format & CLI

A **cross-platform** package format and command-line tool written in Rust.
Builds and installs `.upkg` packages on **Windows**, **Linux**, and **macOS**.
Analogous to Debian's `dpkg` but not tied to any single distribution.

## Credits

- **Author:** Edan
- **Version:** 0.1.0

## Quick start

```bash
# Build from source
cargo build --release

# Create a package from a TOML config file
upkg create myapp.toml

# Verify a package file
upkg verify myapp-linux-64.upkg

# Install locally
upkg install myapp-linux-64.upkg

# Install directly from a URL (streaming on seekable servers)
upkg install https://example.com/myapp-linux-64.upkg /opt/myapp

# Verify an installed folder
upkg verify /opt/myapp --package myapp-linux-64.upkg

# Repair corrupt or missing files
upkg repair /opt/myapp --package myapp-linux-64.upkg

# List installed packages
upkg list

# Show package or installed-app info
upkg info myapp-linux-64.upkg
upkg info myapp

# Remove
upkg remove myapp

# Download a package without installing
upkg download https://example.com/myapp-linux-64.upkg --output /tmp
```

## Create-config format (TOML)

```toml
app-name = "myapp"
app-version = "1.2.3"
os = "linux"              # windows / linux / mac
os-version = "22.04"
min = "20.04"             # optional lower bound
max = "24.04"             # optional upper bound
distro = "ubuntu"         # optional (linux only)
strict = false            # reject on version/distro mismatch
type = "application"      # application / game / data / … (Section 13)
arch = "64"               # optional: 32 / 64 / arm64
compression = "zstd"      # none / zstd
compression-kind = "per-file"   # per-file / whole-archive
compression-level = 3

source = "path/to/files"  # folder whose contents become the package
output = "dist"           # optional output directory

dependencies = ["libc", { name = "foo", min = "1.0", max = "2.0" }]
conflicts = ["old-app"]
replaces = ["legacy-app"]
signing = "my-key.seed"   # optional ed25519 signing key

[shortcut]               # optional; kind: universal / desktop / lnk
kind = "universal"
name = "My App"
exec = "myapp --flag"
```

## Key properties

| Property          | Description                                                    |
| ----------------- | -------------------------------------------------------------- |
| Magic             | `UPKG` (UTF-8, 4 bytes)                                       |
| Header            | Fixed binary block, 17 fields (Section 7.2)                   |
| Hashes            | 4 × SHA-1 (80 bytes): header, master, tree, metadata          |
| Metadata          | dpkg-control-like UTF-8 text file (Section 6)                 |
| Entries tree      | Recursive UTF-8 tree - one NUL-delimited line per entry       |
| File data         | Per-file or whole-archive compressed with zstd or stored raw  |
| Signature         | Optional ed25519 over all preceding bytes                     |
| Compression       | `none` / `zstd`; RAR is forbidden (open-format requirement)   |
| Package database  | JSON + SHA-1 trailer in `<root>/packages/`                    |
| Online streaming  | HTTP Range streaming for per-file packages on seekable hosts  |

## Format encoding proposals

The following implementation details are proposals (the specification leaves them
open):

- **Header encoding** - fixed 45-byte binary prefix (magic + u32/u64/u8 fields)
  followed by nine NUL-terminated UTF-8 strings.
- **Entries tree encoding** - one line per entry; fields delimited by `\0`
  (NUL), records by `\n` (LF); depth-folding for the recursive structure.
- **Signature section** - 7-byte marker `UPKGSIG`, 32-byte ed25519 public key,
  64-byte signature (103 bytes total). The marker makes presence unambiguous.
- **`source`** config key - the directory whose contents become the package.
- **`output`** config key - optional output directory override.
- **`upkg keygen`** - helper to generate ed25519 signing keys.
- **`UPKG_ROOT`** environment variable - overrides the default install root
  (useful for portable installs and tests).
- **`max-download-minutes`** - in the install config file, the configurable
  speed-gate limit for full downloads (default 20 minutes).

## License

GNU General Public License v3.0 (GPL-3.0-only)
