# siox-lsp

Language Server Protocol implementation for
[siox](https://github.com/Siox-lang/sioxc).

The compiler is pinned as the `sioxc/` Git submodule. Clone and build with:

```bash
git clone --recurse-submodules git@github.com:Siox-lang/siox-lsp.git
cd siox-lsp
cargo build
```

Run over standard input/output:

```bash
target/debug/siox-lsp --stdio --std sioxc/std
```

Update the compiler dependency explicitly:

```bash
git -C sioxc fetch
git -C sioxc checkout <compiler-commit>
git add sioxc
```
