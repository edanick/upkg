# UPKG - Universal Package Format & CLI

## 1. Overview

UPKG is a **universal package format** and a **command-line tool** written in **Rust** that creates and installs UPKG packages. It is analogous to `dpkg` (Debian's package manager), but is **not tied to a single operating system or distribution**: UPKG packages work on **Windows**, **Linux** (any distribution), and **macOS** - exactly these three operating systems are supported (identifiers: `windows`, `linux`, `mac` - Section 4).

The format is designed around four principles:

1. **Cross-platform** - one package format for every OS and Linux distribution.
2. **Self-describing** - every package declares its target OS and OS version range in both its metadata file and its binary header.
3. **Integrity-checked** - SHA-1 hashes are stored per file and for the archive as a whole.
4. **Open** - only open-source, freely usable compression algorithms are permitted. Proprietary formats such as **RAR** are forbidden.

## 2. How to read this document

- **Enumerated names**: every name, field, value, and term used in this specification is listed explicitly. Vague catch-alls such as "etc." are avoided; where a list is intentionally open-ended it is written as `...`. Nothing is referenced that is not defined here.
- **Naming rule (from the draft)**: every name that appears in implementation artifacts - comments, file names, folder names, variables, constants, enums, functions, namespaces, modules, interfaces, classes, structs, and any other construct - must be defined in this specification. No artifact may reference a name that is not listed here.
- **`Proposal`**: a detail not fixed by the original draft, introduced to make the spec complete. Proposals are for the author to accept, reject, or change.

## 3. Guiding constraints (source of truth)

These are the format's hard requirements and must never be violated (items 1-6 come from the original draft; items 7-9 were added in revision 2; item 10 in revision 6; item 11 in revision 16; item 12 in revision 26; items 13-15 in revision 27; items 16-17 in revision 28):

1. Every package must declare the **OS** and **OS version** it targets - as a single version or a range using `min` and `max`. `min` without `max` means "since" (at least version X), `max` without `min` means "until" (at most version X), both mean "between", and neither means any version. Both bounds are **inclusive**: `host_version >= min` and `host_version <= max`, both when only one is present and when both are (resolved in revisions 4 and 24 - Section 7.2).
2. Every package must declare its **dependencies** and **requirements**, if any. Dependencies may carry **version bounds** (`min`/`max` - min only, max only, both, or none), reusing the bound semantics of Section 7.2; a dependency is satisfied only when it is installed **and** its version falls within the bounds (resolved in revision 29 - Sections 6, 9).
3. The package **header** must encode the OS and OS version information.
4. Package **file names** are **recommended** to follow a fixed convention (e.g. `app-win-64.upkg`) so the application, OS, and architecture are identifiable at a glance; the convention is not mandatory (resolved in revision 3 - Section 5).
5. Only **open-source, free-to-use compression** is allowed. **RAR is forbidden.**
6. The implementation is written in **Rust**.
7. The package must support **verifying its own integrity** and **verifying an extracted folder** by comparing per-file SHA-1 hashes recursively (Section 10).
8. **Repair** must restore both the **corrupt** files (hash mismatch) and the **missing** files (in the package but absent from the folder) in an extracted folder (resolved in revision 12 - Section 10.3). For `whole-archive` packages, repair requires temporarily decompressing the archive (e.g. to the system temp directory) and is therefore slow; for `per-file` packages, only the affected file bytes are read via their offsets.
9. The format must support **online (streaming) installation**: given a URL and a target folder, the tool downloads only the byte ranges it needs when possible. With a **seekable** host (HTTP Range) and `per-file` compression it downloads the entries tree first, then each file by its data start/end offsets. `whole-archive` packages and non-seekable hosts are downloaded whole (to a temp file), gated by a speed test on non-seekable connections; on refusal the tool prints an error (host does not support seeking and the internet is too slow, or "not possible with this host"), and on success it warns that an interrupted download must be redone (resolved in revisions 14-15 - Section 11).
10. **Install-time OS compatibility** (resolved in revisions 6 and 25 - Section 9): a package for a **completely different OS** is **rejected**; a package for a **different version of the same OS** **warns** and installs anyway, unless the package sets `strict: true` (stored in the metadata file and the header), in which case it is **rejected** too. The same applies to a `linux` package declaring a **`distro`**: a package for a **different Linux distro** (e.g. `distro: ubuntu` on Fedora) **warns** and installs anyway, unless `strict: true`, in which case it is **rejected** too (added in revision 25).
11. **Missing dependencies at install time** (resolved in revisions 16-17 - Section 9): on **Windows**, warn about missing dependencies and continue. On **Linux**, warn, then ask y/n whether to try installing the extra dependencies (warning that this may cause conflicts and system issues); on y, run the system install commands; on n, ask y/n whether to install the package anyway or leave - yes installs without the dependencies (warning that it **might not work at all or properly**), no aborts.
12. **Desktop shortcuts** (added in revision 26 - Sections 4, 6, 9): a package may carry **one** shortcut template - at most one per package. There are three templates: **universal** (adapts to the target OS - `linux` -> `.desktop`, `windows` -> `.lnk`, `mac` -> `.command`), **desktop** (a customizable `.desktop`-format template for `linux`/`mac` packages), and **lnk** (a windows-only `.lnk` template). A template/OS mismatch (`lnk` on a non-windows package, `desktop` on a windows package) is **rejected at create time, before any work starts**. On `linux`, the `.desktop` file is written to `~/Desktop` when that folder exists, otherwise to `~/.local/share/applications`.
13. **Path safety** (added in revision 27 - Sections 7.4, 9): every entry's `relative path` must be **relative** and must never escape the target folder - no `..` components, no absolute paths, no drive letters, no backslashes. `upkg create` **rejects** such paths, and `upkg install` **rejects** any package whose entries violate them before writing anything (zip-slip protection). Windows packages additionally reject reserved device names (`CON`, `NUL`, `AUX`, `PRN`, `COM1`-`COM9`, `LPT1`-`LPT9`) and trailing dots/spaces at create time, and reject names that collide case-insensitively.
14. **Signing** (added in revision 27 - Sections 6, 7.7, 9-10): a package **may** carry an optional **ed25519** signature over all preceding bytes; when present, `upkg verify` and `upkg install` **reject** the package if the signature is invalid.
15. **Conflicts and replacements** (added in revision 27 - Sections 6, 9): a package may declare `conflicts` and `replaces`. Installing a package that **conflicts** with an installed package is **rejected**; a package that **replaces** another removes the replaced package's files and database entry (Section 12).
16. **Package type** (added in revision 28 - Sections 6, 7.2, 13): a package may declare a `type` - `application`, `game`, `album`, `music album`, `pictures`, `documents`, `data`, `database`, or `misc`. The type selects the default install folder (Section 13), which the user can override with custom locations in their own install config. Unknown types fall back to `misc` (proposal).
17. **Architecture** (added in revision 28 - Sections 6, 7.2, 9): a package may declare an `arch` - `32`, `64`, or `arm64`. When the host architecture differs, install **warns** and installs anyway, unless `strict: true`, in which case it is **rejected** (mirroring OS-version handling).

## 4. Terminology

| Term | Definition |
|---|---|
| UPKG | The universal package format and its Rust CLI tool |
| Package | A single file with the `.upkg` extension |
| Header | The fixed metadata block at the start of a package |
| Overall SHA-1 (master) | Archive-level integrity hash stored after the header: over the post-compression archive data for `whole-archive` packages, over the concatenated raw file contents for `per-file` packages (Section 7.3) |
| original SHA-1 | Per-entry hash of the raw (uncompressed) file contents (Section 7.4) |
| post-compression SHA-1 | Per-entry hash of the stored (possibly compressed) bytes, used for per-entry integrity (Section 7.4) |
| tree SHA-1 | A hash of the entire entries tree (its stored bytes), covering tree integrity (Section 7.3) |
| header SHA-1 | A hash of the entire header, detecting a broken header (Section 7.3) |
| Entries tree | The recursive structure describing the folders and files in the package, serialized as a UTF-8 tree (Section 7.4) |
| Entry | One node of the entries tree: a folder or a file |
| OS | Target operating system, exactly one of: `windows`, `linux`, `mac` (resolved in revision 10) |
| OS version | Target OS version, expressed as one value or a range |
| distro | The Linux distribution a `linux` package targets, e.g. `ubuntu`, `debian`; optional - absent means any distro (added in revision 25) |
| min | Inclusive lower bound; without `max`, `min` alone means "since" (at least version X); with both, the version must be between them; with neither, any version (resolved in revisions 4 and 24) |
| max | Inclusive upper bound; without `min`, `max` alone means "until" (at most version X); with both, between them; with neither, any version (resolved in revisions 4 and 24) |
| Compression algorithm | `none`, `zstd`, ... (full list in [Section 8](#8-compression)) |
| Compression kind | `per-file` or `whole-archive` |
| Compression level | Numeric strength of the compression algorithm |
| SHA-1 | The 160-bit (20-byte) hash used for integrity |
| Data start offset | Byte offset where a file's data begins in the data section (Section 7.4) |
| Data end offset | Byte offset where a file's data ends in the data section (Section 7.4) |
| size | Uncompressed size in bytes of a file (Section 7.4) |
| mode | Unix-style file permissions (octal bits) for `linux`/`mac` packages (Section 7.4) |
| attributes | Windows file attributes as true/false flags for `windows` packages (Section 7.4) |
| Online (streaming) installation | Installing from a URL: fetching only the needed byte ranges on seekable hosts, or a speed-gated full download on non-seekable hosts (Section 11) |
| Seekable | An HTTP host that supports Range requests (`206 Partial Content` / `Accept-Ranges: bytes`), allowing partial fetches |
| HTTP Range | The HTTP mechanism for requesting a byte range of a resource |
| strict | Boolean flag in the metadata file and header; when `true`, install rejects on OS-version mismatch instead of warning (added in revision 6) |
| separator | The NUL byte `0x00`, delimiting fields within a record (Section 7.6) |
| line breaker | The LF byte `0x0A`, delimiting records such as entries in the entries tree (Section 7.6) |
| uint | Unsigned integer, stored as fixed-width little-endian bytes - offsets/lengths/sizes are u64, format version and compression level are u32 (Section 7.6) |
| speed test | A quick RAM-only download probe against the target file used to gate unseekable downloads (Section 11.2) |
| active scanner | Progressive monitoring of a temp download that extracts each file as soon as its bytes are complete (Section 11.2) |
| Metadata file | The dpkg-control-like file embedded in the package describing it |
| `upkg` | The CLI tool itself |
| shortcut template | One of three config templates in the metadata file that the installer converts into an OS-native desktop shortcut (added in revision 26) |
| universal shortcut config | A shortcut template that adapts to the target OS: `linux` -> `.desktop`, `windows` -> `.lnk`, `mac` -> `.command` (added in revision 26) |
| desktop shortcut config | A shortcut template mirroring the freedesktop `.desktop` format with its full field set; for `linux`/`mac` packages (added in revision 26) |
| lnk shortcut config | A windows-only shortcut template converted to a `.lnk` file (added in revision 26) |
| .desktop | The freedesktop desktop-entry file format used on Linux desktops (added in revision 26) |
| .lnk | The Windows shortcut file format (added in revision 26) |
| .command | The macOS executable shell-script shortcut file format (added in revision 26) |
| metadata SHA-1 | A hash of the entire metadata file (its stored bytes), covering the embedded config (added in revision 27 - Section 7.3) |
| signature | Optional ed25519 signature over all preceding package bytes, providing authenticity beyond integrity (added in revision 27 - Section 7.7) |
| ed25519 | The public-key signature algorithm used for package signing (added in revision 27 - Section 7.7) |
| conflicts | Other packages this package conflicts with; a conflict with an installed package rejects install (added in revision 27 - Section 6) |
| replaces | Other packages whose files this package replaces; the replaced package's entry is removed from the database (added in revision 27 - Section 6) |
| package database | The on-disk manifest of installed packages, enabling list/status/verify/remove and conflict checks (added in revision 27 - Section 12) |
| type | The package category - `application`, `game`, `album`, `music album`, `pictures`, `documents`, `data`, `database`, `misc` - which selects the default install folder (added in revision 28 - Section 13) |
| arch | The target architecture - `32`, `64`, `arm64`; a host mismatch warns unless `strict` is set (added in revision 28 - Section 9) |
| install config | The user's own config file that remaps type folders to custom locations; never hardcoded in the tool (added in revision 28 - Section 13) |
| dependency bounds | Optional `min`/`max` version bounds on a dependency, same semantics as OS-version bounds (added in revision 29 - Section 6) |
| status | Install state in the database: `unpacked` or `installed` (added in revision 29 - Section 12) |

## 5. Package file naming

The filename convention is based on the original example `app-win-64.upkg`:

```
<app-name>-<os>-<arch>.upkg
```

| Part | Meaning | Example |
|---|---|---|
| `<app-name>` | Application name | `app` |
| `<os>` | OS abbreviation | `win`, `linux`, `mac` |
| `<arch>` | Architecture | `64`, `32`, `arm64` |
| `.upkg` | Fixed extension | `.upkg` |

Notes:

- The OS and architecture are duplicated: once in the **filename** (for humans) and once in the **header** (for machines). The header is authoritative.
- **Resolved (revision 3)**: the filename convention is a **recommendation**, not a requirement. When creating a package, if the user did not provide the version number, OS, or other components, the CLI emits a **warning** listing what was not provided (e.g. "warning: version number was not provided", "warning: OS was not provided").

## 6. Package metadata file

Analogous to the `dpkg` control file, each package embeds a **metadata file** that describes it. The `upkg create` command (see [Section 9](#9-cli-reference-proposals)) generates it from the config supplied by the user.

Required fields:

| Field | Required | Description |
|---|---|---|
| app name | yes | Name of the application |
| app version | yes *(inferred: needed to identify the application build)* | Version of the application |
| OS | yes | Target OS, exactly one of: `windows`, `linux`, `mac` (resolved in revision 10) |
| OS version | yes | Single version or a range: `min`, `max` |
| distro | no *(`linux` only)* | Target Linux distribution, e.g. `ubuntu`, `debian`; absent = any distro (added in revision 25) |
| dependencies | only if any | Other packages this package depends on, each optionally with version bounds: `{name, min?, max?}` - min only, max only, both, or none, with the same semantics as OS-version bounds (Section 7.2) - added in revision 29 |
| requirements | only if any | Other requirements (e.g. runtime, hardware) |
| strict | no *(defaults to `false`)* | When `true`, install rejects on OS-version mismatch instead of warning (added in revision 6 - Section 9) |
| attributes | no *(defaults to `false`; `windows` only)* | When `true`, file entries carry Windows file attributes as true/false flags (added in revision 21) |
| modes | no *(defaults to `false`; `linux`/`mac` only)* | When `true`, file entries carry Unix permission bits (added in revision 21) |
| shortcut | no | At most one shortcut template block (`universal`, `desktop`, or `lnk`) - Section 6, added in revision 26 |
| conflicts | only if any | Other packages this package conflicts with; installing alongside an installed conflicting package is rejected (added in revision 27) |
| replaces | only if any | Packages whose files this package replaces; the replaced package's entry is removed from the database (added in revision 27) |
| signing | no | When set, the package is signed with ed25519 at create time (private key path supplied) - Section 7.7 (added in revision 27) |
| type | no *(defaults to `application`)* | Package type: `application`, `game`, `album`, `music album`, `pictures`, `documents`, `data`, `database`, `misc` - selects the default install folder (added in revision 28 - Section 13) |
| arch | no | Target architecture: `32`, `64`, `arm64` (added in revision 28) |
| description | no | Short one-line summary, optionally followed by a longer description (added in revision 30) |
| homepage | no | Project homepage / bug-report URL (added in revision 30) |
| author | no | Author or maintainer (added in revision 30) |
| license | no | License identifier (SPDX recommended) (added in revision 30) |

When the package is built, the `OS` and `OS version` fields are **encoded into the header** ([Section 7.2](#72-header-fields)). The metadata file itself is embedded in the package as **section 3 of the layout** ([Section 7.1](#71-overall-layout-proposal)), bounded by header fields 14-15 and covered by the **metadata SHA-1** ([Section 7.3](#73-hashes-header-sha-1-master-sha-1-tree-sha-1-metadata-sha-1)) - added in revision 27.

If the user omits any of these fields when running `upkg create`, the CLI does not fail silently: it **warns**, listing which fields were not provided (version number, OS, and so on - resolved in revision 3, Section 5).

`description`, `homepage`, `author`, and `license` are **informational** fields stored only in the metadata file - they are not encoded into the header and do not affect install behavior; they are shown by `upkg info` (Section 9) and are useful for future `upkg search` and repository listings (added in revision 30).

### Shortcut templates (added in revision 26)

The config may include **at most one** `shortcut:` block (constraint 12). The installer converts it into an OS-native desktop shortcut at install time (Section 9). Three templates exist, selected by the `kind` field:

**Universal** (allowed on any OS; adapts to the target):

| Field | Description |
|---|---|
| kind | `universal` |
| name | Shortcut name |
| exec | Command line that launches the application |
| icon | Optional icon path |
| comment | Optional description |
| working-directory | Optional working directory |

Adaptation: `linux` -> a `.desktop` file (`Name`, `Exec`, `Icon`, `Comment`, `Path`, `Type=Application`) written to `~/Desktop` if it exists, else `~/.local/share/applications`; `windows` -> a `.lnk` file on the user's Desktop; `mac` -> a `.command` executable shell script on `~/Desktop`.

**Desktop** (allowed on `linux`/`mac` packages; mirrors the freedesktop `.desktop` format for full customization):

| Field | Description |
|---|---|
| kind | `desktop` |
| name | `Name=` |
| comment | `Comment=` |
| exec | `Exec=` |
| icon | `Icon=` |
| type | `Type=` (default `Application`) |
| categories | `Categories=` |
| terminal | `Terminal=` boolean |
| mime-type | `MimeType=` |
| path | `Path=` working directory |
| generic-name | `GenericName=` |
| keywords | `Keywords=` |
| no-display | `NoDisplay=` boolean |

On `linux` it is written to `~/Desktop` if it exists, else `~/.local/share/applications`; on `mac` to `~/Desktop`.

**LNK** (windows packages only; converted to a `.lnk` file):

| Field | Description |
|---|---|
| kind | `lnk` |
| target | Full path of the executable the shortcut points to |
| arguments | Command-line arguments |
| working-directory | Working directory |
| icon-location | Icon source path (`.exe`/`.ico`) |
| icon-index | Icon index within the icon source |
| description | Tooltip text |
| window-style | `normal`, `minimized`, or `maximized` |
| hotkey | Optional hotkey *(proposal)* |
| run-as-admin | Optional boolean *(proposal)* |

Validation, enforced **before any work starts** (constraint 12): `lnk` requires a `windows` package; `desktop` requires a `linux` or `mac` package; `universal` is allowed on any OS. At most one `shortcut:` block per config - more than one is a create-time error. Each template also requires its core fields (`universal`: `name` + `exec`; `desktop`: `name` + `exec`; `lnk`: `target`) - a missing core field is a create-time error.

The generated shortcut file is named `<name>` plus the OS extension - `<name>.desktop`, `<name>.lnk`, or `<name>.command` (invalid filename characters in `name` are sanitized at create time). Conversion details (proposals): for `windows`/`lnk`, the universal `exec` command line is split into `.lnk` `target` (first token) and `arguments` (the rest); for `mac`, the `.command` file contains `#!/bin/sh` followed by `exec <exec>` and is created executable.

## 7. Package file format

### 7.1 Overall layout (proposal)

A package is one file with six ordered sections (the signature is optional and always last):

```
+----------------------------------+
| 1. Header                        |  fixed metadata block (Section 7.2)
+----------------------------------+
| 2. Hashes (hdr+master+tree+md)  |  80 bytes, header + master + tree +
|                                  |  metadata SHA-1 (Section 7.3)
+----------------------------------+
| 3. Metadata file                 |  the embedded config (Section 6), bounds
|                                  |  recorded in the header (fields 14-15)
+----------------------------------+
| 4. Entries tree                  |  folders + files (Section 7.4), bounds
|                                  |  recorded in the header (fields 10-11)
+----------------------------------+
| 5. File data                     |  the (compressed) file contents (Section 7.5)
+----------------------------------+
| 6. Signature (optional)          |  ed25519 over all preceding bytes (Section 7.7)
+----------------------------------+
```

### 7.2 Header fields

The header is the fixed metadata block at the start of the package. Field order within the header is a proposal.

| # | Field | Type | Required | Description |
|---|---|---|---|---|
| 1 | Magic | 4 bytes (UTF-8) | yes | UTF-8 encoded `UPKG` - identifies the format (resolved in revision 8; strings are UTF-8 per Section 7.6) |
| 2 | Format version | u32 | yes | Version of the UPKG format itself |
| 3 | OS | string / enum | yes | Exactly one of: `windows`, `linux`, `mac` (resolved in revision 10) |
| 4 | OS version | string | yes | Target OS version |
| 5 | min | version string | no | Lower bound; if `max` is not used, `min` alone means "since" (at least this version); with both, the version must be between them; with neither, any version |
| 6 | max | version string | no | Upper bound; if `min` is not used, `max` alone means "until" (at most this version); with both, between them; with neither, any version |
| 7 | Compression algorithm | enum | yes | `none`, `zstd`, ... (Section 8) |
| 8 | Compression kind | enum | yes | `per-file` or `whole-archive` |
| 9 | Compression level | u32 | yes, unless algorithm is `none` | Algorithm-dependent (e.g. `1`-`22` for zstd) |
| 10 | Entries tree start offset | u64 | yes | Byte offset where the entries tree begins |
| 11 | Entries tree end offset | u64 | yes | Byte offset where the entries tree ends; tree length = end - start |
| 12 | strict | boolean | no *(defaults to `false`)* | If `true`, install rejects on OS-version or distro mismatch instead of warning (added in revision 6) |
| 13 | distro | string | no *(`linux` only)* | Target Linux distro, e.g. `ubuntu`, `debian`; absent = any distro (added in revision 25) |
| 14 | Metadata file start offset | u64 | yes | Byte offset where the metadata file begins (added in revision 27) |
| 15 | Metadata file end offset | u64 | yes | Byte offset where the metadata file ends; length = end - start (added in revision 27) |
| 16 | type | string / enum | no *(defaults to `application`)* | Package type: `application`, `game`, `album`, `music album`, `pictures`, `documents`, `data`, `database`, `misc` - selects the default install folder (added in revision 28 - Section 13) |
| 17 | arch | string / enum | no | Target architecture: `32`, `64`, `arm64` (added in revision 28) |

A range is expressed by combining the required `OS version` field with the optional `min`/`max` bounds; a single version uses `OS version` alone. `min` without `max` means "since" (at least this version); `max` without `min` means "until" (at most this version).

The bounds are all **optional** (resolved in revision 23): with **neither** bound present, the package installs on **any version**; with **only `min`**, only the lower bound applies; with **only `max`**, only the upper bound applies; with **both**, the version must be **between** them.

The bounds are all **inclusive** (resolved in revision 24): a single `min` means `host_version >= min`; a single `max` means `host_version <= max`; both means `host_version >= min && host_version <= max`; neither means the package installs on any version.

The entire header block is covered by the **header SHA-1** stored after it (Section 7.3).

### 7.3 Hashes (header SHA-1, master SHA-1, tree SHA-1, metadata SHA-1)

Four 20-byte hashes are stored immediately after the header. Together they cover every part of the package except the hash block itself, which cannot hash itself (resolved in revisions 11, 18, 19, and 27):

| Hash | Covers |
|---|---|
| **Header SHA-1** | The **entire header** (Section 7.2) - detects a broken header (added in revision 19) |
| **Master SHA-1** | The file data - input depends on the compression kind: `whole-archive` = the **post-compression** (stored) archive data; `per-file` = the concatenated **raw (uncompressed)** file contents (resolved in revisions 5 and 11) |
| **Tree SHA-1** | The **entire entries tree** (its stored bytes) - covers tree integrity (added in revision 18) |
| **Metadata SHA-1** | The **entire metadata file** (its stored bytes, Section 6) - covers the embedded config (added in revision 27) |

### 7.4 Entries tree

- A **recursive structure**: a folder entry contains child entries (folders and files).
- Its byte range is bounded by the **start** and **end** offsets stored in the header (fields 10 and 11).
- The original draft asked "how does it look: relative path, filename, sha1, etc." - the answer is the enumerated field list below.
- Each file entry carries the **original SHA-1** (raw contents, resolved in revision 5) and, for `per-file` packages, a **post-compression SHA-1** over the stored bytes - integrity is checked per entry (resolved in revision 11).
- The entries tree is a **UTF-8 tree** - a recursive UTF-8 text structure using separators and line breakers - located at the byte range given by the uint start offset and end offset in the header (fields 10-11); tree length = end - start (resolved in revisions 7 and 9 - Sections 7.4 and 7.6).

| Field | Description | Status |
|---|---|---|
| entry type | `folder` or `file` | required (from the draft) |
| relative path | Path relative to the package root | required (from the draft) |
| filename | Base name of the file | required (from the draft) |
| SHA-1 (original) | Per-file hash of the raw (uncompressed) file contents | required (from the draft) |
| SHA-1 (post-compression) | Per-file hash of the stored (possibly compressed) bytes - tells which files are valid (per-entry integrity) | required when compression kind is `per-file` *(added in revision 11)* |
| data start offset | Byte offset where this file's data begins in the data section | required when compression kind is `per-file` *(added in revision 2: enables repair and online install)* |
| data end offset | Byte offset where this file's data ends in the data section | required when compression kind is `per-file` *(added in revision 2)* |
| size | Uncompressed size in bytes (u64) | required *(added in revisions 20 and 27)* |
| mode / attributes | File permissions (`mode`, Unix bits) or file attributes (true/false flags), per the target OS - see the note below | required when enabled in the config *(added in revisions 20-21)* |

Offset semantics (proposal): data start/end offsets span the **stored (possibly compressed)** bytes of the file and are **absolute byte offsets from the start of the package file**, matching header fields 10-11.

Mode/attribute semantics (resolved in revisions 20-21): the field is named and encoded per the package's target OS (header field 3) - for `linux` and `mac` it is **mode**: the Unix-style permission bits (octal, e.g. `0755`); for `windows` it is **attributes**: boolean true/false flags (e.g. read-only, hidden, archive), since Windows does not use Unix permissions. The field is stored only when the config enables it (Section 6).

Path safety (added in revision 27 - constraint 13): every `relative path` must be **relative** (no leading `/`, no drive letters), must not contain `..` components or backslashes, and must never resolve outside the package root; `upkg create` rejects violations, and `upkg install` rejects any package whose entries violate them before writing anything.

### 7.5 File data

The file contents, stored according to the compression kind ([Section 8](#8-compression)):

- `per-file`: each file is compressed individually with the declared algorithm and level. Each file occupies a contiguous byte range in the data section, given by its data start/end offsets (Section 7.4), so any file can be read independently (used by repair and online install).
- `whole-archive`: all files are compressed together as a single stream.

### 7.6 Encoding rules (resolved in revisions 7, 9, and 22)

The package file combines a **binary header** with a **UTF-8 entries tree**:

- **Unsigned integers (uints)**: integers, offsets, and lengths - including the entries tree and metadata file start/end offsets (header fields 10-11 and 14-15) - are stored as raw bytes in fixed-width **uint** fields, **little-endian** (resolved in revisions 22 and 27). Widths (resolved in revision 27): all **offsets, lengths, and sizes are u64 (8 bytes)**; the **format version** and **compression level** are u32 (4 bytes); **booleans** (e.g. `strict`) are a single byte - `0x00` = false, `0x01` = true.
- **UTF-8**: all strings - OS, OS version, app name, app version, relative paths, filenames, and the **entries tree itself** - are UTF-8 encoded. The entries tree is a **UTF-8 tree** whose byte range is given by the uint offset/length fields in the header (Section 7.4).
- **Separators**: fields within a record are delimited by the **NUL byte `0x00`** (resolved in revision 22).
- **Line breakers**: records (e.g. entries in the entries tree) are delimited by the **LF byte `0x0A`** (resolved in revision 22).

String values must not contain the separator byte (`0x00`) or the line-breaker byte (`0x0A`) - `upkg create` rejects (or escapes) such values, e.g. filenames containing LF/CR/NUL.

### 7.7 Signature (ed25519, optional - added in revision 27)

The package may be **signed** for authenticity, as opposed to integrity, which the SHA-1 hashes provide:

- The signature is an **ed25519** signature over **all preceding bytes of the package file** (header + hashes + metadata file + entries tree + file data), stored as the **last section** (Section 7.1).
- The section contains the signature plus the **public key** it was made with (proposal: a 32-byte ed25519 public key followed by a 64-byte signature).
- Signing is enabled at create time via the `signing` config option (Section 6). The public key is embedded in the package; verifying trust in that key is the user's responsibility (a keyring / trust-on-first-use model is a proposal).
- `upkg verify` and `upkg install` verify the signature whenever it is present and **reject** the package if it is invalid (Sections 9-10).

## 8. Compression

| Rule | Value |
|---|---|
| Allowed algorithms | `none`, `zstd` |
| Candidate additions *(proposal)* | `gzip`, `lz4`, `xz`/`lzma`, `bzip2` |
| License rule | Open-source and free to use only |
| Forbidden | `rar` and any proprietary format |
| Kinds | `per-file`, `whole-archive` |
| Level | Integer; meaning depends on the algorithm (e.g. zstd accepts `1`-`22`) |

Streaming (Section 11.1) requires the `per-file` kind; `whole-archive` packages are always downloaded whole (Section 11).

## 9. CLI reference (proposals)

The draft specifies that the tool can **create** and **install** packages. The exact command names below are proposals.

| Command | Purpose | Status |
|---|---|---|
| `upkg create <config>` | Build a `.upkg` package from a metadata config file | from the draft |
| `upkg install <file.upkg>` | Install a package from a local file | from the draft |
| `upkg install <url> <folder>` | Online (streaming) install from a URL - Section 11 | added in revision 2 |
| `upkg verify <file.upkg>` | Verify the integrity of the upkg file itself - Section 10.1 | added in revision 2 |
| `upkg verify <folder> --package <file.upkg>` | Recursively verify an extracted folder against the package's per-file SHA-1 hashes - Section 10.2 | added in revision 2 |
| `upkg repair <folder> --package <file.upkg>` | Restore corrupt and missing files in an extracted folder - Section 10.3 | added in revision 2 |
| `upkg info <file.upkg>` | Print the package's metadata and header | proposal |
| `upkg remove <app>` | Remove an installed application: its files, its package database entry, and its generated desktop shortcut - Section 12 | added in revision 27 |
| `upkg download <url> [--output <dir>]` | Download a `.upkg` to disk without installing (offline bundles, caching); default output is the current directory | added in revision 29 |

Install-time OS compatibility (resolved in revision 6):

- Package targets a **completely different OS** (e.g. package is for `windows`, host is `linux`) -> **reject**; do not install.
- Package targets a **different version of the same OS** (e.g. another Windows version, another Linux version) -> **warn** and install anyway.
- If the package's `strict` flag is `true` (metadata + header, Sections 6 and 7.2), the version mismatch is also a **reject** instead of a warning.
- A `linux` package may declare a **`distro`** (e.g. `distro: ubuntu`, `distro: debian` - Section 6 and header field 13); a package for a **different Linux distro** -> **warn** and install anyway; with `strict: true`, **reject** with an error instead (added in revision 25). Host distro detection on Linux is a proposal: read the `ID` field of `/etc/os-release`.
- A package may declare an **`arch`** (e.g. `64`, `arm64` - Section 6 and header field 17) that differs from the host -> **warn** and install anyway; with `strict: true`, **reject** instead (added in revision 28). Host architecture detection is a proposal: `x86_64` -> `64`, `i686` -> `32`, `aarch64` -> `arm64`.

Install-time dependency handling (resolved in revisions 16-17):

- **Windows**: warn about the missing dependencies and continue installing.
- **Linux**: warn about the missing dependencies; then ask y/n whether to try installing them, warning that this **may cause conflicts and system issues**:
  - **y** -> run the system install commands for the extra dependencies;
  - **n** -> ask y/n whether to install the package **anyway or leave**:
    - **y** -> install anyway, without the dependencies, and warn that it **might not work at all or properly**;
    - **n** -> abort (leave).

Versioned dependencies (added in revision 29): a dependency with `min`/`max` bounds is **missing** when it is not installed or when the installed version falls outside the bounds; the same warn / y-n flow above applies.

Install-time shortcut generation (added in revision 26): if the package's metadata file contains a `shortcut:` template (Section 6), the tool generates the OS-native shortcut after the files are installed:

| Template | Windows | Linux | macOS |
|---|---|---|---|
| universal | `.lnk` on the user's Desktop | `.desktop` on `~/Desktop`, falling back to `~/.local/share/applications` | `.command` on `~/Desktop` |
| desktop | - *(rejected at create time)* | `.desktop`, same placement as universal | `.desktop` on `~/Desktop` |
| lnk | `.lnk` on the user's Desktop | - *(rejected at create time)* | - *(rejected at create time)* |

The generated file is named `<name>` plus the OS extension, and the `.command` file is created executable. Template/OS mismatches never reach install: they are rejected at create time (Section 6).

Install-time safety checks, all performed **before any files are written** (added in revision 27):

- **Signature** (constraint 14): when the package carries a signature (Section 7.7), install verifies it and **rejects** the package on invalid signature.
- **Path safety** (constraint 13): install rejects any package whose entries violate the path safety rules (Section 7.4) - zip-slip protection.
- **Conflicts** (constraint 15): install **rejects** a package that conflicts with an installed package (Section 12); a package that **replaces** another first removes the replaced package's files and database entry, then installs.

## 10. Verification & repair

### 10.1 Verify the upkg file itself

`upkg verify <file.upkg>` checks that the package file is intact. The integrity level depends on the compression kind (resolved in revisions 11, 18, 19, and 27):

- the **header SHA-1** (Section 7.3) over the entire header must match;
- the **metadata SHA-1** (Section 7.3) over the metadata file must match;
- the **tree SHA-1** (Section 7.3) over the entries tree must match;
- `whole-archive` - **archive level**: the master SHA-1 (Section 7.3) over the post-compression archive data must match; reaching per-file data requires decompressing the whole archive (same cost as repair, Section 10.3).
- `per-file` - **per-entry level**: for each entry, the stored bytes are checked against the **post-compression SHA-1** (which tells whether the file is valid), then decompressed and checked against the **original SHA-1**; the master SHA-1 over the raw contents must also match.
- when a **signature** is present (Section 7.7), it must verify over all preceding bytes; otherwise the package is rejected.

Together these hashes cover the header, the metadata file, the entries tree, and the file data; only the hash block itself is not covered, since it cannot hash itself.

### 10.2 Verify an extracted folder

`upkg verify <folder> --package <file.upkg>` compares an already installed/extracted folder against the package:

1. Recursively walks the folder.
2. Computes the SHA-1 of every file found.
3. Compares each hash to the per-file **original** SHA-1 in the entries tree.
4. Reports **corrupt** files (hash mismatch) and **missing** files (in the package but absent from the folder). Reporting **extra** files (in the folder but not in the package) is a proposal.

### 10.3 Repair an extracted folder

`upkg repair <folder> --package <file.upkg>` restores both the **corrupt** files (original SHA-1 mismatch) and the **missing** files (in the package but absent from the folder), writing the correct bytes from the package (resolved in revision 12).

Cost depends on the compression kind:

- `per-file`: only the affected files are read, using their data start/end offsets. Fast.
- `whole-archive`: any single file requires decompressing the entire archive, so the tool extracts to the default system temp directory first, then swaps the affected files. Slow for large archives (acknowledged trade-off).

## 11. Online installation (streaming)

A package can be installed from a URL without downloading the whole `.upkg` file first when the connection is **seekable**; non-seekable hosts fall back to a full download gated by a speed test (resolved in revision 14).

Requirements (both modes):

- Streaming requires the `per-file` compression kind (Section 7.5); a `whole-archive` package is always downloaded whole.
- Every file entry must carry data start/end offsets (Section 7.4).
- The entries tree must be a UTF-8 tree (resolved in revisions 7 and 9 - Section 7.6) located at the byte range recorded in the header (Section 7.2 fields 10-11).

Both modes work with **any file size**: bytes are streamed to disk, never held in RAM.

### 11.1 Seekable connection (HTTP Range supported)

The tool probes seekability first (proposal: an HTTP `HEAD` request checking `Accept-Ranges: bytes`, optionally confirmed with a small `Range` probe). If the host supports Range requests (responds `206 Partial Content` and/or advertises `Accept-Ranges: bytes`), streaming is used:

- `per-file` (separately compressed): download the header + entries tree (one HTTP Range request), then download each file's bytes using its data start/end offsets and **extract each file immediately into the destination folder - no temp file**.
- `whole-archive` (summed compression): download the **entire archive into a temp file**; if the download fails, **resume from the downloaded length and append** to the temp file; install after everything has been downloaded.

### 11.2 Unseekable connection (no HTTP Range)

Partial downloads are impossible, so the tool falls back to downloading the **entire file**, after a speed gate:

1. **Speed test**: run an internet speed test against the target file first, reading into RAM; **close that socket** without really downloading with it, then **reopen a new socket** for the real download.
2. **Speed gate** - proceed only within these limits, so we do not risk a slow connection that can easily get cut off:
   - speed **under 1 MB/s**: refuse if the estimated download time is above **5 minutes** (fixed - cannot be configured). On refusal the tool prints an **error**: the host does not support seeking and the internet is too slow.
   - speed **1 MB/s or more**: refuse if the estimated download time is above **20 minutes** (configurable). On refusal the tool reports **"not possible with this host"**.
   - When the speed gate passes, the tool prints a **warning** before downloading: the host does not support seeking, and if the download fails and gets interrupted it must be redone.

**Overwrite rule**: when (re)starting a temp download, the temp file is **deleted and recreated** (seek to 0 on a fresh file) rather than seeking to 0 in an existing file - for safety, never write into leftover data from a previous attempt.

Then, per compression kind:

- `per-file`: download the entire file to temp with an **active scanner** that detects when the entries tree is complete and when each file's bytes have been appended, **extracting each file to the destination progressively**. If the connection interrupts, restart **from offset 0** (delete + recreate the temp file) and re-download until reaching the previous point again.
- `whole-archive`: download the entire file to temp; if it fails, **restart and overwrite** the file.

Security note (proposal): the bytes come from an untrusted URL, so the tool must verify them against the package's SHA-1 hashes before writing them into the target folder - the **post-compression SHA-1** while streaming (no decompression needed), then the **original SHA-1** after decompression, and the **master SHA-1** at the end (Sections 7.3-7.4).

## 12. Package database

On every successful install, the tool records the package in an on-disk **package database** (added in revision 27), so installed software can be listed, verified, removed, and checked for conflicts without the original `.upkg` file:

| Stored per package | Description |
|---|---|
| app name | Key of the database entry (one installed version at a time) |
| app version | Installed version |
| OS / OS version | As declared in the package |
| install path | Where the files were placed |
| files | List of installed file paths with their original SHA-1s |
| shortcut | Whether a desktop shortcut was generated (Section 6) |
| dependencies | Declared dependencies, for dependency checks |
| status | Install state - `unpacked` or `installed` - enabling crash recovery (added in revision 29) |

- Storage location (proposal): `~/.local/share/upkg/packages/` on `linux`, `%LOCALAPPDATA%\upkg\packages\` on `windows`, `~/Library/Application Support/upkg/packages/` on `mac`; one file per package (JSON or TOML - proposal).
- Enabled by the database: `upkg list` / `upkg info` on installed apps, `upkg verify <folder>` without `--package`, `upkg remove <app>` (Section 9), and the install-time conflict check (Section 9).

Install transaction (added in revision 29): install proceeds in order - (1) write all files, (2) record the database entry with status `unpacked`, (3) verify every file against its SHA-1, (4) mark the entry `installed`. If the process is interrupted, the entry stays `unpacked`; the next `upkg install` or `upkg repair` detects it and completes verification (re-writing mismatched or missing files) before marking it `installed`.

Database integrity (added in revision 29): each database file ends with a SHA-1 of its contents (proposal); the tool verifies it on every read and reports tampering. Signing the database with the user's own key is a future option.

## 13. Install locations (by type)

The optional `type` field (Section 6, header field 16) determines where a package's files are installed (added in revision 28):

| type | Default folder |
|---|---|
| `application` | `apps` |
| `game` | `games` |
| `album` | `music` |
| `music album` | `music` |
| `pictures` | `pictures` |
| `documents` | `documents` |
| `data` | `data` |
| `database` | `databases` |
| `misc` | `misc` |

Default install root per OS (proposal): `~/.local/share/upkg/` on `linux`, `%LOCALAPPDATA%\upkg\` on `windows`, `~/Library/Application Support/upkg/` on `mac`. The final default path is `<root>/<type folder>/<app name>` - e.g. a `game` on Windows installs to `%LOCALAPPDATA%\upkg\games\<app name>`.

The mapping is **configurable**: the user can remap any type folder to a custom location in their own **install config** (proposal: `~/.config/upkg/install.toml` on `linux`, `%APPDATA%\upkg\install.toml` on `windows`, `~/Library/Application Support/upkg/install.toml` on `mac`), e.g. `games = "D:\\Games"`. Such values come from the user's config - the tool never hardcodes drive-letter or custom paths; an unmapped type uses its default folder. The resolved install path is recorded in the package database (Section 12).

