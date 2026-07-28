# siox-lsp

Language Server Protocol implementation for
[siox](https://github.com/Siox-lang/sioxc).

The backend-independent compiler core is fetched directly by Cargo from the
`sioxc` Git repository:

```bash
git clone git@github.com:Siox-lang/siox-lsp.git
cd siox-lsp
cargo build
```

Run over standard input/output:

```bash
target/debug/siox-lsp --stdio --std /path/to/sioxc/std
```

`--std` points at the SIOX standard-library installation or a `sioxc`
checkout. `Cargo.lock` pins the exact compiler revision used by this LSP build.
