# CityOS native extension

This package holds CityOS integration behavior independently of the desktop editor. It is a native Rust extension; the existing HTML plugin sandbox does not provide the trusted document, history and preview-window interfaces required by bound drafts.

The current continuation reuses upstream `desigma-engine` for generic editing, layout, component synchronization, resize and variable operations. CityOS semantic history policy lives in `src/history.rs` over the core's generic staged transition and immutable process-local identity fence. Independent restores receive fresh identities; unchanged clones retain their marker. This is source reuse in primary `main`, not upstream Git ancestry integration. Typed document bindings/container and submission-policy/storage still need further isolation.

Generic library prerequisites now include root OS transaction exclusion, expected-checkpoint comparison, complete ordinary checkpoint copies and Windows extended-path atomic replacement. The native host tracks persisted history independently of visible document changes and protects managed files from external-save bypasses. `target/cityos-persistence-routing-check-fixed-20261018/native-compilation.json` records current locked/offline Rust 1.93.1 test-source checking; current executable and packaged qualification are tracked separately under `target/cityos-persistence-final-qualification-20261018/`. The preceding 122-pass library stage predates later copy/routing changes. These records do not close Windows, full persistence, owner acceptance, estate coverage or performance gates. Earlier snapshots below remain historical within their declared scope.

Dependency direction:

```text
desigma-app → desigma-cityos → desigma-doc / desigma-library / desigma-cityos-catalog
```

The extension must not import application internals or include source from the app. `scripts/cityos-boundary.test.mjs` checks this direction and the shared scheduler boundary. The read-only catalog wire client remains in `desigma-cityos-catalog`.

## Package and host responsibilities

The extension owns discovery browsing, selection/provenance checks, data specimens, local theme-library construction, exchange IPC, projection construction, preview hosting, dependency retention calls, layout preferences and the CityOS job budget. Catalog UI receives the editor's style and recording services. Temporary specimen rendering uses a host interface rather than owning the active editor.

The app hosts the CityOS navigation entry and supplies native canvas rendering. The 18 October continuation extracts the Workspace controller and UI into `desigma-cityos::workspace`; `desigma-app/src/design_workspace` retains the `Workspace<App>` alias and native save/history/canvas/session adapter. The package and native app, including their test sources, pass locked offline Rust 1.93.1 typechecking in `target/cityos-editor-host-all-check-adapters-20261018`. The current 11 boundary/upstream checks pass in `target/cityos-editor-host-boundary-final-20261018.log`. Executable regression and packaged qualification remain separate gates. This extraction does **not** claim complete elimination of CityOS coupling.

The extracted source uses `Workspace<H: EditorHost>` with the port in `src/editor.rs` and an opaque `H::Session`. The native session privately owns document/history, file/library identities, autosave and view state. The package keeps resource/scenario/semantic selection state. The port does not expose mutable `App`, `History`, `Autosave` or `Library` references; qualification of the current implementation remains pending.

The host API must make these boundaries concrete:

- `switch` saves and flushes before moving state. A rejected switch leaves both active and saved sessions intact; consume a saved slot only after success. Detached preview sessions have no save target.
- `install` distinguishes presentation-only replacement, a validated semantic edit/history step, and a durable baseline. Baseline causes distinguish reviewed incoming reconciliation from the original accepted submission record. Presentation does not change semantic dirty state, history or retry bookkeeping.
- An uncertain durable write returns an explicit recovery identity covering file, draft, local revision, semantic digest and the complete binding digest, including base and dependencies. It blocks editing, saving, switching and close until authoritative local readback resolves it; it is never treated as rejection. Original accepted-submission readback and dependency cleanup remain available.
- Status and available actions are immutable DTOs. Save, undo/redo, open, portable export and native canvas rendering remain host operations. Canvas interaction returns semantic selection and permits only inspection/pan/zoom outside qualified commands.

Keep real-host tests for failed-save switching, complete session restoration, autosave/close, dirty hashing, selection, persistence and protected input paths. Package tests should inject host rejection, success and uncertainty while checking response fences, history receipts and recovery ordering.

The initial native session seam is implemented in `desigma-app/src/design_workspace/session.rs`. `App::replace_document_transaction` invokes its ownership-transfer closure only after the final save succeeds. Saved tabs contain an opaque `EditorSession`; failed switches leave both active history and the incoming slot owned by their original holders. Session transfer moves history instead of cloning it, and restores native selection/view alongside file and autosave state. Workspace opening, local copies and saved-tab activation use this transaction. The preceding app stage executed three session/save-switch and two pending-notes regressions within 903 passing tests, with seven ignored. The earlier pending-session-test wording is superseded. Those results do not verify the later generic `EditorHost` controller/UI extraction or runtime qualification.

Document container compatibility, atomic checkpoints, generic portable import/export and persistence recovery remain in the document/library crates. Those generic improvements should be proposed upstream independently. CityOS-specific document/history binding and submission-policy/storage coupling in core still require a separately reviewed extension boundary. Upstream engine integration remains open. Existing GPU and plugin changes are preserved independently of this package.

The next storage prerequisite introduces generic `Library::payloads(namespace)` / `PayloadStore` in `desigma-library/src/payloads.rs`. It owns exact-byte hashing, verified reads, no-op retention and atomic repair. The existing CityOS methods delegate to that store with the unchanged `cityos/payloads/<sha256>` layout. Namespace and link checks constrain extension-owned paths. Authorization, portable-export policy, dependency roots, garbage collection and submission/synchronization transactions stay with their existing owners. This is not the final submission-store extraction. The library, CityOS package and app test sources pass locked offline Rust 1.93.1 typechecking together in `target/cityos-extension-storage-all-check-20261018`; executable verification remains pending. Its library build deferred before compilation at the 8 GiB commit guard in `target/cityos-extension-storage-library-tests-20261018`. The supplemental prebuild/full-source readback there records unchanged inputs and membership. These results are separate from the preceding host-port checkpoint.

## Updating upstream

Run from the existing `main` checkout:

```powershell
node scripts/upstream-audit.mjs --fetch --output=target/upstream-audit.json
node --test scripts/upstream-audit.test.mjs scripts/cityos-boundary.test.mjs
cargo +1.93.1 test -p desigma-cityos --features open-url --locked --offline -j 1
```

The audit fetches only the upstream remote, records immutable identities and computes a merge simulation. It does not merge, switch branches, stage files or touch the checkout/index. The output target must be new so prior evidence cannot be overwritten. Without `--fetch`, the report describes the currently fetched upstream ref.

The 18 October audit in `target/cityos-upstream-audit-20261018-host-port.json` records a successful fetch at fork head `afef859d34ed49909ec0a4075bed922d299e7d22` against upstream `db4a3cdcac0a2445d2d5ee06c79ef5451c2405ab`: 28 commits ahead, 11 behind, with merge base `127936b41a8b9da30644fdacd047fc54f7ad30c9`. Its committed-head simulation reports three conflicts: `crates/desigma-app/Cargo.toml`, `crates/desigma-app/src/comp_ops.rs` and `crates/desigma-app/src/edit.rs`. Dirty source is fingerprinted separately; this audit does not integrate or verify the current extraction.

Resolve every reported conflict and review upstream's shared-engine changes before an authorized integration. Textual merge success does not qualify behavior. Re-run document/library, native app and packaged journeys with the matching reader/bridge/renderer; preserve generic editing guards when upstream moves operations into `desigma-engine`. Keep dirty work intact and serialize shared builds. Never use `ours` or `theirs` over whole editing, GPU, plugin or persistence files.

For upstream `db4a3cdc`, the two editing conflicts are small but consequential: `Edit::node` rejects ordinary mutation of bound CityOS documents, and component command analysis handles the whole-document install command. Upstream moved those implementations into `desigma-engine`. Resolve the hooks in their new owner and prove protected alternate input paths still fail; retaining only the app's re-exports would lose the protection or fail an exhaustive command match.

## Qualification boundary

The preceding extraction stage records 111 package passes and two ignored checks in `target/cityos-extension-tests-20261018/extension-tests.log`; both ignored checks separately pass with the pinned packaged runtime in `pinned-bridge-tests.log` in that directory. `target/cityos-switch-notes-fixed-20261018/app-tests.log` records 903 app passes and seven ignored, including the session/save-switch and notes regressions; its `notes-tests.log` also records the two notes passes separately. `target/cityos-portable-library-qualified-20261017/library-tests.log` records 108 library passes and three ignored. These are preceding-stage evidence, not compilation or test results for the current generic Workspace source.

The current generic host port passes test-source typechecking, but its executable package test build deferred before starting a compiler after the 8 GiB available-commit guard remained unmet for 183 seconds. `target/cityos-editor-host-package-tests-20261018/native-compilation.json` records that result. The eight new controller and three native host regressions are not executed evidence yet. The earlier interrupted wait has a separate observation in `target/cityos-editor-host-all-check-20261018/interrupted-observation.json`; its stale waiting report is not a live compiler or a compilation result.

CMS candidate `20261017T215836010Z-bf93fbf389a44de797ead2863710d652` is staged, not activated. CMS `.build/desigma-expanded-evidence/extension-candidate-stage-20261018/candidate-stage.json` records the unchanged active pointer and stable launcher. The matching packaged bridge headless frame receipt in CMS `.build/desigma-expanded-evidence/extension-candidate-bound-frame-20261018/native-roundtrip.log` records zero unchanged operations, one edited operation and zero after undo, with `embeddedRendererObserved: false`. The stable installed launcher remains on build `20261017T165146000Z-b72d6657a3b34abcb95b4eaf3186a26e`.

Canonical `cityos.design-workspace.verify` executes 137 passing tests with zero failures/skips, but exits 16 because source changed during execution and its proof was superseded. CMS `.build/desigma-expanded-evidence/extension-canonical-workspace-20261018/result.json` records the command result and exact evidence/receipt paths. It is not a current passing canonical receipt.

Package tests prove the functions they exercise. They do not establish physical Windows WebView2 accessibility, GPU coexistence, owner acceptance, whole-estate rendering or the full performance budget. Keep the previous installed desktop active until a matching package has passed the required checks.
