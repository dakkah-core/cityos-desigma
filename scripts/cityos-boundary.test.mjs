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

// Inspect declarations, leaving the adapter's real Rust tests in place. Mask
// strings/comments before balancing test modules so JSON and format strings
// cannot hide production code that follows a test module.
function productionRust(source) {
  const shape = source.replace(/\/\/[^\n]*|\/\*[\s\S]*?\*\/|r(#{0,})"[\s\S]*?"\1|"(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])'/g,
    value => value.replace(/[^\n]/g, ' '));
  const output = shape.split('');
  const modules = /#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+\w+\s*\{/g;
  for (let match; (match = modules.exec(shape));) {
    let depth = 1;
    let end = modules.lastIndex;
    while (depth && end < shape.length) {
      if (shape[end] === '{') depth++;
      if (shape[end] === '}') depth--;
      end++;
    }
    assert.equal(depth, 0, 'Unclosed Rust test module');
    output.fill(' ', match.index, end);
    modules.lastIndex = end;
  }
  return output.join('');
}

function assertEditorPort(source) {
  const declarations = productionRust(source);
  assert.match(declarations, /\bpub\s+trait\s+EditorHost\b/);
  const signatures = [...declarations.matchAll(/\bfn\s+(\w+)\b([^;{}]*);/g)];
  assert.ok(signatures.length, 'EditorHost must expose operation signatures');
  for (const [, name, signature] of signatures) {
    const returns = signature.slice(signature.indexOf('->') + 2);
    if (!signature.includes('->')) continue;
    assert.doesNotMatch(returns, /&\s*(?:'\w+\s+)?mut\b/, `${name} returns mutable host state`);
    assert.doesNotMatch(returns, /\b(?:App|History|Library)\b/, `${name} returns an editor-owned handle`);
  }
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

test('Workspace controller belongs to the package and is generic over EditorHost', () => {
  const library = productionRust(readFileSync(join(extension, 'src/lib.rs'), 'utf8'));
  assert.match(library, /\bpub\s+mod\s+editor\s*;/);
  assert.match(library, /\bpub\s+mod\s+workspace\s*;/);
  const controller = productionRust(readFileSync(join(extension, 'src/workspace.rs'), 'utf8'));
  const declaration = controller.match(/\bpub\s+struct\s+Workspace\s*<([^{};]+)>\s*(?:where\s+([^{};]+))?\s*\{/);
  assert.ok(declaration, 'The package must own a generic Workspace controller');
  assert.match(`${declaration[1]} ${declaration[2] ?? ''}`, /\b\w+\s*:\s*(?:[\w:]+::)?EditorHost\b/);
  assert.doesNotMatch(controller, /\b(?:App|desigma_app)\b/, 'Workspace must use the editor port');
});

test('EditorHost exposes operations without returning mutable editor state or storage handles', () => {
  assertEditorPort(readFileSync(join(extension, 'src/editor.rs'), 'utf8'));
});

test('semantic history policy belongs to the extension over a generic core transition', () => {
  const core = productionRust(readFileSync(join(root, 'crates/desigma-doc/src/history.rs'), 'utf8'));
  assert.doesNotMatch(core, /\b(?:struct\s+CityosHistoryStep|fn\s+(?:prepare_cityos|commit_cityos|cityos_step_current))\b/);
  assert.match(core, /\bpub\s+struct\s+HistoryTransition\b/);
  assert.match(core, /\bpub\s+fn\s+commit_transition\b/);
  const policy = productionRust(readFileSync(join(extension, 'src/history.rs'), 'utf8'));
  assert.match(policy, /\bimpl\s+CityosHistory\s+for\s+History\b/);
  assert.match(policy, /\.commit_transition\s*\(/);
});

test('portable owner policy stays in the extension while the library exposes an explicit generic port', () => {
  const portable = productionRust(readFileSync(join(root, 'crates/desigma-library/src/portable.rs'), 'utf8'));
  const generic = productionRust(readFileSync(join(root, 'crates/desigma-library/src/storage_policy.rs'), 'utf8'));
  const policy = productionRust(readFileSync(join(extension, 'src/storage_policy.rs'), 'utf8'));
  assert.doesNotMatch(portable + generic, /\b(?:CityosBase|CityosBinding|NativeOwner)\b/,
    'Shared storage must not interpret owner-specific portability or dependency contracts');
  assert.match(generic, /\bpub\s+trait\s+CheckpointStoragePolicy\b/);
  assert.match(policy, /\bimpl\s+CheckpointStoragePolicy\s+for\s+CityosStoragePolicy\b/);
  assert.match(policy, /\bCityosBase::NativeOwner\b/);
  assert.match(portable, /\bpolicy\.check_portable\s*\(/);
});

test('app workspace is an alias and host adapter, without a production controller copy', () => {
  const directory = join(root, 'crates/desigma-app/src/design_workspace');
  const adapter = productionRust(readFileSync(join(directory, 'mod.rs'), 'utf8'));
  assert.match(adapter, /\bdesigma_cityos::workspace\b/);
  assert.match(adapter, /\btype\s+Workspace\s*=\s*[\w:]*Workspace\s*<[^;]+>\s*;/);
  for (const path of files(directory)) {
    const source = productionRust(readFileSync(path, 'utf8'));
    assert.doesNotMatch(source, /\bstruct\s+(?:Workspace|DraftTab)\b/, path);
    assert.doesNotMatch(source, /\bimpl\s*(?:<[^{};]*>\s*)?Workspace\b/, path);
  }
});

test('boundary checks detect nested mutable returns and production code after retained tests', () => {
  const port = method => `pub trait EditorHost { ${method} }`;
  assert.doesNotThrow(() => assertEditorPort(port("fn swap(&mut self, slot: &mut Option<Self::Session>) -> Result<Self::Session, String>; fn document(&self) -> &Document;")));
  for (const returns of ["&mut App", "Option<&'a mut desigma_doc::History>", 'Result<&mut desigma_library::Library, String>', 'Box<App>', 'Library']) {
    assert.throws(() => assertEditorPort(port(`fn state(&mut self) -> ${returns};`)), assert.AssertionError);
  }
  const source = '#[cfg(test)] mod tests { struct Workspace; fn fixture() { let json = r#"{ }"#; } }\nstruct Workspace;';
  const production = productionRust(source);
  assert.equal([...production.matchAll(/\bstruct\s+Workspace\b/g)].length, 1);
  assert.match(production, /struct Workspace;/);
  assert.doesNotMatch(productionRust('#[cfg(test)] pub(crate) mod fixtures { use desigma_doc::CityosBase; }\nfn production() {}'), /CityosBase/);
  assert.match(productionRust('#[cfg(test)] pub(crate) mod fixtures { struct TestOnly; }\nstruct CityosBase;'), /struct CityosBase;/);
});
