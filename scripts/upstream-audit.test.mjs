import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyPath, inputDigest, parseMergeTree, sanitizeDiagnostic } from './upstream-audit.mjs';

test('clean and conflicted merge-tree reports keep exact conflict paths', () => {
  const tree = 'a'.repeat(40);
  assert.deepEqual(parseMergeTree(`${tree}\n`, 0), { tree, clean: true, conflicts: [] });
  const conflict = `${tree}\r\ncrates/desigma-app/Cargo.toml\r\ncrates/desigma-app/src/edit.rs\r\n\r\nAuto-merging edit.rs\r\n`;
  assert.deepEqual(parseMergeTree(conflict, 1), { tree, clean: false, conflicts: ['crates/desigma-app/Cargo.toml', 'crates/desigma-app/src/edit.rs'] });
  for (const [output, status] of [['fatal: no objects', 128], ['wrong-tree', 0], [`${tree}\nfile.rs`, 0]]) {
    assert.throws(() => parseMergeTree(output, status));
  }
});

test('package extraction is distinct from residual hooks and preserved concurrent changes', () => {
  for (const [path, expected] of [
    ['crates/desigma-cityos/src/bridge.rs', 'cityos-package'],
    ['crates/desigma-cityos-catalog/src/lib.rs', 'cityos-package'],
    ['crates/desigma-app/src/design_workspace/mod.rs', 'cityos-editor-adapter'],
    ['crates/desigma-library/src/portable.rs', 'document-and-persistence'],
    ['crates/desigma-gpu/src/exec.rs', 'preserve-gpu'],
    ['crates/desigma-app/src/plugins/mod.rs', 'preserve-plugins'],
    ['crates/desigma-app/src/edit.rs', 'editor-hook-review'],
    ['Cargo.lock', 'build-manifest'], ['unknown.rs', 'review-required'],
  ]) assert.equal(classifyPath(path), expected, path);
});

test('authentication diagnostics never print embedded URL credentials', () => {
  const message = 'fatal: https://name:secret@example.test/source and http://actor:credential@host/repo';
  assert.equal(sanitizeDiagnostic(message), 'fatal: https://<redacted>@example.test/source and http://<redacted>@host/repo');
  assert.equal(sanitizeDiagnostic('https://opaque-user-carrier@example.test/repo'), 'https://<redacted>@example.test/repo');
});

test('capture identity detects same-status tracked and untracked content changes', () => {
  const status = ' M existing.rs\0?? new.rs\0';
  const original = inputDigest(status, 'first diff', ['new.rs', 'first contents']);
  assert.notEqual(inputDigest(status, 'second diff', ['new.rs', 'first contents']), original);
  assert.notEqual(inputDigest(status, 'first diff', ['new.rs', 'second contents']), original);
  assert.notEqual(inputDigest(status, 'first diff', ['new.r', 'sfirst contents']), original);
  assert.equal(inputDigest(status, 'first diff', ['new.rs', Buffer.from('first contents')]), original);
});
