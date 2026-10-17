import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const extension = join(root, 'crates/desigma-cityos');
function files(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? files(path) : path.endsWith('.rs') ? [path] : [];
  });
}

test('CityOS package has no reverse dependency or source inclusion into the editor', () => {
  const manifest = readFileSync(join(extension, 'Cargo.toml'), 'utf8');
  assert.doesNotMatch(manifest, /desigma-app/);
  for (const path of files(join(extension, 'src'))) {
    const source = readFileSync(path, 'utf8');
    assert.doesNotMatch(source, /(?:use\s+crate::app|desigma_app::|desigma-app\/|#\[path\s*=\s*"[^"\n]*\.\.\/)/, path);
  }
});

test('catalog and exchange share the extension scheduler, with no second editor budget', () => {
  const adapter = readFileSync(join(root, 'crates/desigma-app/src/cityos_jobs.rs'), 'utf8');
  assert.match(adapter, /pub(?:\(crate\))? use desigma_cityos::jobs::\*/);
  assert.doesNotMatch(adapter, /static\s+(?:HEAVY|INTERACTIVE|PRESSURE)/);
  const bridge = readFileSync(join(extension, 'src/bridge.rs'), 'utf8');
  assert.match(bridge, /crate::jobs::run/);
  const browser = readFileSync(join(extension, 'src/catalog/mod.rs'), 'utf8');
  assert.match(browser, /crate::jobs::run/);
});

test('Windows preview implementation belongs to the package while app imports its host API', () => {
  const host = readFileSync(join(extension, 'src/host.rs'), 'utf8');
  assert.match(host, /wry::/);
  const appManifest = readFileSync(join(root, 'crates/desigma-app/Cargo.toml'), 'utf8');
  assert.doesNotMatch(appManifest, /^wry\s*=/m);
  const adapter = readFileSync(join(root, 'crates/desigma-app/src/design_workspace/host.rs'), 'utf8');
  assert.match(adapter, /desigma_cityos::host/);
  assert.doesNotMatch(adapter, /WebViewBuilder|evaluate_script/);
});
