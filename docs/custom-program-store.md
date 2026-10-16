<a id="повторное-использование-custom-rust"></a>

# Reusing Custom Rust

[English](custom-program-store.md) | [Русский](ru/custom-program-store.md) | [Documentation](README.md)

`CompiledProgramStore` retains immutable compiled code between Game, Scene and Rust hot-apply preparations. The goal is to avoid rebuilding the same worker after Stop while keeping game runs independent. This is a separate layer above ordinary Cargo dependencies; persistent game state still belongs to the host, not the cache.

Nothing needs enabling in the normal Editor: start Play, press Stop and start again. With unchanged verified inputs, ready code is reused and game state starts afresh. Preparation still checks files and invokes Cargo, so a repeat launch need not be instantaneous. When Editor closes completely, store memory is released; the next process can reuse verified SDK dependencies on disk but prepares the program root again.

<a id="владение-и-подключение"></a>

## Ownership and integration

`GameWorkbench` owns one `Arc<CompiledProgramStore>`. Stop releases the game host/actor and retains the store. Each successful compiler request, including a cache hit, creates a new `ScriptInstance`: separate state, channel and worker. `Scene` and `Game` do not receive a shared mutable instance. The worker process itself starts on the instance's first call. For compatible hot apply, state transfer is governed by the existing host contract described in [custom-state.md](custom-state.md).

For your own host, create an `Arc<CompiledProgramStore>` and pass it to `ScriptCompiler::with_program_store` or `GameHost::build_cancellable_with_store`. Previous constructors remain available. The editor's CLI wrapper creates a local store; SDK dependencies can use the same verified disk namespace in the next process.

`CompiledScript::clone` retains its previous shared-instance semantics. For an independent run, an ordinary clone cannot replace a new store request. A store entry contains only metadata, a build recipe and artifact leases, not a live `ScriptInstance`.

<a id="что-проверяется"></a>

## What is checked

Before reuse, content checks cover Custom source and mode, Core/SDK, Cargo locks, the actual dependency/feature graph, the supported native toolchain, target, Cargo configuration and selected compiler/link inputs. The root key also accounts for the effective environment and working paths. The absence of a significant config/toolchain file is part of the check too: such a file appearing must change the decision. File modification time is not proof of unchanged content.

A successful entry contains actual Cargo JSON artifacts, dep-info, generated worker inputs and the executable. A warm hit first compares the proof, then runs Cargo `--offline --locked` in its own saved workspace. A real `compiler-artifact` with `fresh=true` for the root binary is required; a counter does not simulate Cargo's response. Inputs are checked again afterward. A change during preparation rejects the candidate while preserving the previous running code. These are before/after checks, not an atomic filesystem snapshot or a guarantee against arbitrary external ABA changes.

A separate persisted SDK receipt checks outputs before Cargo can use the old dependency namespace. It covers SDK products, build-script outputs and control files, fingerprints and compiler-query cache. This is required for both new source and a new editor process: proof of sources does not prove that an old `.rlib` is intact. An interprocess namespace lease covers verification, Cargo, rechecking and receipt publication. A missing or invalid receipt with existing products requires a new SDK generation; a directory marker alone is insufficient.

The SDK directory uses a short locator, `sdk-v3/<16 hex>`, to conserve the Windows linker path budget. Identity remains the full 64-character SHA-256 in the marker, receipt and memory keys. If short names collide for different keys, the other namespace is neither accepted nor overwritten: a new owned generation is used. A very deep selected target can still hit native linker limits; use a short absolute `CARGO_TARGET_DIR`.

A previously unknown `include!`/`include_str!`/`include_bytes!` may require an initial uncached build to obtain actual dependencies from dep-info. The next preparation checks these files in advance. A new root generation does not borrow potentially stale root products. If SDK inputs are unverified, a new owned SDK generation is used. A corrupt or missing executable is not a hit. A foreign or unverified cache directory is neither taken into ownership nor deleted.

<a id="поддержанная-область-доказательства"></a>

## Supported proof scope

This is a conservatively verified native build subset, not support for an arbitrary Cargo workspace or a hermetic OS copy. Registry dependencies are resolved only for a pinned audited set: the source tree is compared against a digest obtained from the `.crate` archive after checking its checksum against the worker lock. Changes to the audited dependency list require reinspection and an update to `freshness/audited_dependencies.rs`.

Supported routes are stock Windows MSVC and Linux x86_64 GNU/GCC linkers verified by the native resolver. Hash/inventory includes selected compiler/linker files, native libraries and significant search directories in their order. Unknown wrappers, custom linker/target flags, unverified build scripts/proc macros, native FFI inputs and other ambiguous settings disable reuse. Ordinary compilation remains available, but uses a new owned generation instead of a questionable old Cargo cache.

For reuse, Cargo home must be known as an absolute path. An unspecified default home and relative `CARGO_HOME` are not considered proven: otherwise Cargo could read a config absent from the snapshot. In these cases ordinary compilation remains available without reuse. The compiler environment itself is not rewritten to pass the check.

For MSVC, reuse supports ordinary COM/registry discovery and a developer shell with explicit `VSCMD_ARG_TGT_ARCH`. A legacy shell without target arch and discovery requiring fallback `vswhere.exe` remain uncached: the library used there launches its own unbounded subprocesses, which our resolver does not accept. The restriction applies only to the discovery environment copy; the real Cargo/compiler environment is preserved. This does not guarantee bounded time for all system COM/registry calls.

Registry digests are updated with `scripts/generate-audited-custom-dependencies.ps1` in PowerShell 7.3+. Supply `-LockPath` for a real generated Custom worker, `-CargoHome` with local `.crate` archives and a separate `-EvidenceOutput`. The script neither downloads packages nor runs Cargo. It checks archive checksums against the lock, safe regular-file paths and canonical tree framing, then writes a Rust table and JSON receipt. The full workspace lock is unsuitable for this operation. Changing the expected package count through `-ExpectedPackageCount` does not replace manual review of the new dependency set, build scripts and proc macros. Verification of the current generator reproduced all 23 package digests without changes; the output file is stored separately from the production table.

For the stock Linux DXC environment, a narrow `LD_LIBRARY_PATH` case is allowed: one canonical directory, exactly two regular files `libdxcompiler.so` and `libdxil.so`, with no symlinks, nested directories or additional entries. The path, content and membership are checked. Arbitrary loader overrides, in particular `LD_PRELOAD`/`LD_AUDIT`, receive no such exception. The environment is not cleared to manufacture a cache hit.

Linux loader policy applies on Linux. The Windows MSVC loader does not use `LD_LIBRARY_PATH` or `DYLD_*`; their presence alone does not prohibit Windows reuse. The complete effective root environment is still checked, including values read by user `env!`/`option_env!`. Cargo adds its own loader directories when running tests. Linux runtime cache assertions run in a separate test child with explicitly recognized Cargo additions; the production environment is not rewritten. The Workbench cache regression is marked as explicit native acceptance and runs directly from the compiled test binary in an ordinary native environment, separately from the general Cargo suite.

Changing GUI variables such as DISPLAY should not rebuild the entire SDK: the dependency namespace is separate from the full root environment. The new root context is nevertheless checked separately. Cross-target Custom is still rejected: the executable must be native to the current host.

<a id="лимиты-и-освобождение"></a>

## Limits and release

Defaults: 16 entries, 256 MiB of runnable executable bytes, four simultaneously retained SDK contexts and two simultaneously admitted preparations. Admission covers tool probes, metadata, hashing and Cargo, not just the build itself. Identical concurrent requests are coalesced; waiting checks cancellation. Limits are configured through `ProgramStoreLimits` when creating the store.

LRU evicts entries beyond the limit. Active instances and export retain independent artifact leases: evicting an entry does not delete an executable in use. The last owner passes only verified owned products to cleanup. A preparation that is cancelled, fails or changes during build publishes no cache entry.

The 256 MiB limit applies to executable bytes of store entries. Shared Cargo dependencies on disk, artifacts held by active instances and supporting build products are excluded. This is not a total target-directory size limit. The mechanism does not run a global `cargo clean` or fix freshness by changing SDK timestamps.

<a id="диагностика-и-проверки"></a>

## Diagnostics and checks

`stats()` returns an atomic snapshot without file IO, source reads or acquiring the build mutex. `program_hits`, `cache_misses`, `cargo_build_invocations`, root/dependency built/fresh, eviction/coalescing, resident limits, rejection reasons and proof/build time distinguish reuse from a real rebuild. The snapshot includes neither environment values nor Custom text; ordinary compiler diagnostics retain their previous format. `root_artifacts_built` counts the ordinary worker binary, not its build script.

Runtime checks cover instance independence, real fresh events, cancellation/coalescing, bounded eviction/leases and product corruption. The production Workbench regression separately checks Game → Scene → Stop → Game, initial state/time, a new PID and unchanged authored document. The native Editor benchmark checks the entire user route with the previous overall 45-second watchdog; store counters were added to the report without weakening its criteria. Results of specific runs and known limitations are in [review-fixes-20261016.md](review-fixes-20261016.md).
