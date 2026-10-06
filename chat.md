# Coordination log

## 2026-10-06 — Codex — compiler-backed first LSP

The existing main branch contains an invocation-only stub. Own the LSP stdio
transport, document synchronization, compiler adapter, editor features and
protocol tests in this repository. Reuse `siox::compiler` with in-memory input
and LLVM disabled; update Cargo's compiler lock to the published revision.
Implement live structured diagnostics, hover, definition/type navigation,
references/highlights, document symbols, basic completion and comment-safe
formatting. Verify framed stdio sessions and a headless Neovim client. Do not
change compiler syntax or introduce package discovery/incremental compilation.
The compiler's current documentation lists features that were never present
in this stub; reconcile those claims after verification. Preserve unrelated
compiler AGENTS/chat/.codex changes. No push without a fresh request.

## 2026-10-06 — Codex — server-only completion gate

The user explicitly removes VS Code/Neovim work from scope: focus on a working
server. Own the existing adapter/protocol tests and server-only CI/README.
Remove the uncommitted editor client prototype and editor smoke tests from
this delivery; keep direct framed binary tests as the integration boundary.
Do not change compiler syntax or pipeline. Builds remain in the compiler repo's
guarded services after the verified OOM containment fix. Audit every advertised
capability, document first-version limits, then commit only owned LSP changes.

## 2026-10-06 — Codex — server sign-off

Final guarded job siox-job-3000-1791315418494705615-3652265.service is terminal:
SubState=exited, Result=success, ExecMainStatus=0, MemoryPeak=272818176 bytes.
It fetched the exact published compiler revision from Cargo.lock
(53173bc679d639cfdfe31ffdb07eea4f9c3c9fcf), used that checkout's core/std,
and passed cargo +1.90 build/test/clippy --all-targets -D warnings/fmt --check.
All 14 tests pass: four framing/UTF-16 tests and ten real stdio binary sessions.
The dependency tree contains neither inkwell nor llvm-sys. Tested binary hash:
fbe9f36cede97fb8549b8bb4f4564c94229bf76902a3886348c2b5c88e3d68fc.

Capability audit: transport/lifecycle and malformed requests recover correctly;
unsaved full/incremental edits are atomic, sequential, versioned and UTF-16;
compiler errors, warning codes/help and secondary locations are retained;
repair/close clears stale diagnostics without erasing another open buffer.
Resolved declaration IDs drive local/std/qualified definitions, typed hover,
named-type navigation, shadow-safe references/highlights and flat/hierarchical
outlines. Structs, views, traits, entity ports and named processes are covered.
Completion uses compiler declarations/imports/token spellings, avoids comments
and literals, and handles multibyte prefixes. Compiler formatting is idempotent,
keeps macros and declines comments/error source. CLI errors stay off stdout;
both std-directory and compiler-checkout roots work. No language pipeline fork.

The published resolver does not identify port/member uses for definition;
return null rather than invent a spelling-based binding. Port type navigation
and outlines do work. README/TODO explicitly retain synchronous recompilation,
disk-loaded dependencies, no unsaved dependency overlay/index/cancellation,
comment-preserving formatting and broader semantic queries as follow-ups.
Editor packaging/testing is outside scope per the user's latest instruction.
Only owned server, tests, dependency lock, docs and server-only CI are committed;
no push without a fresh request. Compiler docs are reconciled separately.
