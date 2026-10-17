# CityOS native extension

This package holds CityOS integration behavior independently of the desktop editor. It is a native Rust extension; the existing HTML plugin sandbox does not provide the trusted document, history and preview-window interfaces required by bound drafts.

Dependency direction:

```text
desigma-app → desigma-cityos → desigma-doc / desigma-library / desigma-cityos-catalog
```

The extension must not import application internals or include source from the app. `scripts/cityos-boundary.test.mjs` checks this direction and the shared scheduler boundary. The read-only catalog wire client remains in `desigma-cityos-catalog`.

## Package and host responsibilities

The extension owns discovery browsing, selection/provenance checks, data specimens, local theme-library construction, exchange IPC, projection construction, preview hosting, dependency retention calls, layout preferences and the CityOS job budget. Catalog UI receives the editor's style and recording services. Temporary specimen rendering uses a host interface rather than owning the active editor.

The app hosts the CityOS navigation entry and supplies native canvas rendering. Bound-workspace orchestration currently still resides in `desigma-app/src/design_workspace`: its `App`, save/history, tab and recovery interactions require a further editor-host interface. This extraction does **not** claim complete elimination of CityOS coupling.

The next extraction uses `Workspace<H: EditorHost>` with an opaque `H::Session`. The session privately owns native document/history, file/library identities, autosave and view state. The package keeps resource/scenario/semantic selection state. It does not receive mutable `App`, `History`, `Autosave` or `Library` references.

The host API must make these boundaries concrete:

- `switch` saves and flushes before moving state. A rejected switch leaves both active and saved sessions intact; consume a saved slot only after success. Detached preview sessions have no save target.
- `install` distinguishes presentation-only replacement, a validated semantic edit/history step, and a durable baseline. Baseline causes distinguish reviewed incoming reconciliation from the original accepted submission record. Presentation does not change semantic dirty state, history or retry bookkeeping.
- An uncertain durable write returns an explicit recovery identity. It blocks editing, saving, switching and close until authoritative local readback resolves it; it is never treated as rejection.
- Status and available actions are immutable DTOs. Save, undo/redo, open, portable export and native canvas rendering remain host operations. Canvas interaction returns semantic selection and permits only inspection/pan/zoom outside qualified commands.

Keep real-host tests for failed-save switching, complete session restoration, autosave/close, dirty hashing, selection, persistence and protected input paths. Package tests should inject host rejection, success and uncertainty while checking response fences, history receipts and recovery ordering.

The initial native session seam is implemented in `desigma-app/src/design_workspace/session.rs`. `App::replace_document_transaction` invokes its ownership-transfer closure only after the final save succeeds. Saved tabs contain an opaque `EditorSession`; failed switches leave both active history and the incoming slot owned by their original holders. Session transfer moves history instead of cloning it, and restores native selection/view alongside file and autosave state. Workspace opening, local copies and saved-tab activation use this transaction. The complete generic `EditorHost` port and runtime qualification remain pending; added host regressions are not passing evidence until executed.

Document container compatibility, atomic checkpoints, generic portable import/export and persistence recovery remain in the document/library crates. Those generic improvements should be proposed upstream independently. CityOS-specific binding/history fields and submission storage still require a separately reviewed extension boundary. Existing GPU and plugin changes are preserved independently of this package.

## Updating upstream

Run from the existing `main` checkout:

```powershell
node scripts/upstream-audit.mjs --fetch --output=target/upstream-audit.json
node --test scripts/upstream-audit.test.mjs scripts/cityos-boundary.test.mjs
cargo +1.93.1 test -p desigma-cityos --features open-url --locked --offline -j 1
```

The audit fetches only the upstream remote, records immutable identities and computes a merge simulation. It does not merge, switch branches, stage files or touch the checkout/index. The output target must be new so prior evidence cannot be overwritten. Without `--fetch`, the report describes the currently fetched upstream ref.

Resolve every reported conflict and review upstream's shared-engine changes before an authorized integration. Textual merge success does not qualify behavior. Re-run document/library, native app and packaged journeys with the matching reader/bridge/renderer; preserve generic editing guards when upstream moves operations into `desigma-engine`. Keep dirty work intact and serialize shared builds. Never use `ours` or `theirs` over whole editing, GPU, plugin or persistence files.

For upstream `db4a3cdc`, the two editing conflicts are small but consequential: `Edit::node` rejects ordinary mutation of bound CityOS documents, and component command analysis handles the whole-document install command. Upstream moved those implementations into `desigma-engine`. Resolve the hooks in their new owner and prove protected alternate input paths still fail; retaining only the app's re-exports would lose the protection or fail an exhaustive command match.

## Qualification boundary

Package tests prove the functions they exercise. They do not establish physical Windows WebView2 accessibility, GPU coexistence, owner acceptance, whole-estate rendering or the full performance budget. Keep the previous installed desktop active until a matching package has passed the required checks.
