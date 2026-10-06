# LSP follow-ups

Remaining extensions for the synchronous compiler-backed server.

- Compiler query/cache support before background analysis, cancellation and
  coalescing rapid edits; measure latency and memory on large designs first.
- Loaded-source overlays and dependency invalidation across unsaved buffers;
  package/module discovery belongs to the future project tool.
- Scope/member/import completion and function signatures through semantic APIs,
  not spelling-based approximations.
- Reliable port/field/method reference identities and expansion provenance before
  safe workspace rename, macro expansion views or quick fixes.
- Semantic tokens, signature help/inlay hints, workspace symbols, folding and
  selection ranges when supported by compiler source mappings.
- Comment-preserving compiler formatting before formatting commented source.
- Installation/package tooling for matching `core/` and `std/`; editor distribution
  and Marketplace publication after the client/server packaging is ready.
