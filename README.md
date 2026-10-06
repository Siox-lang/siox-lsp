# siox-lsp

Language Server Protocol implementation for
[siox](https://github.com/Siox-lang/sioxc).

The server uses `siox::compiler::Compiler` directly: unsaved source → parse →
resolve → type check → elaboration/IR diagnostics. It never shells out to
`sioxc`, generates native code, or runs the design. The compiler is a Cargo Git
dependency with LLVM disabled; `Cargo.lock` pins its revision.

## Build and run

```bash
git clone git@github.com:Siox-lang/siox-lsp.git
cd siox-lsp
cargo build --locked
```

Run over standard input/output:

```bash
target/debug/siox-lsp --stdio --std /path/to/sioxc/std
```

`--std` accepts the standard-library directory or a compiler checkout (default
`./std`). Its `std/` and sibling `core/` must match the locked compiler revision.
Cargo does not install those language-source libraries beside the binary.
Analysis can read compile-time fixture files through the compiler. Use it on
trusted source; the server is not a sandbox.

## First-version features

- Diagnostics on open, change and save, with compiler codes, notes, help and
  related locations; stale diagnostics clear on repair/close.
- Full and incremental unsaved edits, ordered versions and UTF-16 positions,
  including non-BMP text and percent-encoded file paths.
- Hover with resolved identity and checked expression type.
- Go to definition, including std; go to type definition for named values.
- References/highlights within the selected compilation's loaded source set;
  compiler declaration IDs distinguish shadowed names.
- Document outline (structs, views, enums, entity ports, impl members/processes,
  functions, traits, constants and aliases). Flat-symbol fallback for older clients.
- Basic top-level/import/keyword completion.
- Canonical whole-document formatting via the compiler. Comments or source
  errors yield no edit; formatting retains original macros/imports, not expansion
  output. Printer style is fixed, so editor indentation options are not applied.

This is a synchronous first version, not an incremental compiler or package
manager. Each open buffer retains one compilation and each accepted change
recompiles it. Dependencies load from disk; there is no overlay for other
unsaved files, workspace/package index, live dependency watcher or asynchronous
cancellation. Navigation only covers bindings/types the compiler exposes. No
guessed member completions, rename, signature help, semantic tokens, inlay
hints, quick fixes, folding or selection ranges are advertised yet. Only local
`file://` documents are supported. Port/member uses without a compiler-resolved
identity return no definition or references; checked named types can still be
navigated. UTF-16 columns past the line end clamp to that end, while invalid
lines and edits splitting surrogate pairs are rejected. See [TODO.md](TODO.md).
The transport rejects headers over 8 KiB and messages over 64 MiB; malformed
framing terminates the connection rather than guessing where the next frame starts.

## Verification

```bash
# SIOX_STD defaults to ../siox/std in the test harness, not in the server.
export SIOX_STD=/path/to/sioxc/std
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Rust tests spawn the binary and check framing/lifecycle, malformed messages,
unsaved diagnostics, Unicode edits, versions, scoped navigation, std navigation,
completion, outlines, and safe/idempotent formatting. These tests use direct LSP
sessions, not editor plugins or GUI automation. Editor-specific packaging and
validation are outside this server-only delivery. CI retrieves libraries at the
exact compiler commit from `Cargo.lock`; no LLVM installation is needed.
