import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync, mkdirSync, mkdtempSync, writeFileSync, realpathSync, lstatSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { qualify } from './verify-native-candidate.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
export function nativeBuildEnvironment(input, test) {
  for (const key of ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_TARGET_DIR', 'RUSTC']) assert.ok(!input[key], 'Unrecorded Rust build override: ' + key);
  for (const key of Object.keys(input)) assert.ok(!key.startsWith('CARGO_PROFILE_'), 'Unrecorded Cargo profile override: ' + key);
  // The native test crate is large enough to exhaust Windows commit space with
  // full debug information. Keep its assertions and code unchanged; record the
  // explicit debug-information setting alongside the actual compiler artifact.
  return { ...input, ...(test ? { CARGO_PROFILE_TEST_DEBUG: '0' } : {}) };
}
export function selectNativeArtifact(log, test, targetRoot) {
  const artifacts = log.split(/\r?\n/u).flatMap(line => {
    try { const value = JSON.parse(line); return value.reason === 'compiler-artifact' && value.target?.name === 'desigma'
      && value.target.kind?.includes('bin') && value.profile?.test === test && typeof value.executable === 'string' ? [value.executable] : []; }
    catch { return []; }
  });
  const unique = [...new Set(artifacts)]; assert.equal(unique.length, 1, 'Exactly one actual Cargo native artifact required');
  const executable = path.resolve(unique[0]), relative = path.relative(path.resolve(targetRoot), executable);
  assert.ok(relative && !relative.startsWith('..') && !path.isAbsolute(relative), 'Cargo artifact must stay within the selected fork target');
  return executable;
}
export function assertNativeTestResourceProfile(log) {
  const artifacts = log.split(/\r?\n/u).flatMap(line => {
    try { const row = JSON.parse(line); return row.reason === 'compiler-artifact' ? [row] : []; } catch { return []; }
  });
  const library = artifacts.filter(row => row.target?.name === 'vello_cpu' && row.target.kind?.includes('lib'));
  const tests = artifacts.filter(row => row.target?.name === 'desigma' && row.target.kind?.includes('bin') && row.profile?.test === true);
  assert.equal(library.length, 1, 'Actual vello_cpu compiler profile required');
  assert.equal(tests.length, 1, 'Actual native test compiler profile required');
  assert.equal(library[0].profile.opt_level, '0', 'The bounded test profile must avoid the failing optimized vello_cpu build');
  for (const row of [library[0], tests[0]]) {
    assert.equal(row.profile.debuginfo, 0);
    assert.equal(row.profile.debug_assertions, true, 'Resource settings must preserve debug assertions');
    assert.equal(row.profile.overflow_checks, true, 'Resource settings must preserve overflow checks');
  }
  return library[0].profile;
}
export async function qualifyNativeBuild(cmsRoot, fixtureRoot) {
  cmsRoot = realpathSync(cmsRoot); fixtureRoot = realpathSync(fixtureRoot);
  assert.equal(process.versions.node, JSON.parse(readFileSync(path.join(cmsRoot, 'config/toolchains/node-typescript.json'))).node);
  const toolchainBytes = readFileSync(path.join(root, 'rust-toolchain.toml'));
  const rustVersion = toolchainBytes.toString().match(/^channel\s*=\s*"(\d+\.\d+\.\d+)"/mu)?.[1];
  assert.ok(rustVersion, 'Exact canonical Rust toolchain required');
  const { acquireWorkspaceValidationLease } = await import(pathToFileURL(path.join(cmsRoot, 'scripts/toolchains/workspace-validation-lease.mjs')).href);
  const release = acquireWorkspaceValidationLease({ timeoutMs: 55000 });
  let output, observedSource;
  const checks = [];
  try {
    const parent = path.join(root, '.build/upstream'); mkdirSync(parent, { recursive: true });
    output = mkdtempSync(path.join(parent, 'native-build-'));
    const git = args => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8', windowsHide: true, maxBuffer: 4 * 1024 * 1024 }).trim();
    const observe = () => {
      const refs = [...new Set(git(['ls-files', '-z', '-c', '-o', '--exclude-standard', '--', 'crates', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo']).split('\0').filter(Boolean))].sort();
      const inputs = refs.map(ref => { const file = path.join(root, ref); assert.ok(lstatSync(file).isFile() && !lstatSync(file).isSymbolicLink()); return { ref, sha256: sha(readFileSync(file)) }; });
      return { commit: git(['rev-parse', 'HEAD']), dirty: git(['status', '--porcelain']).length > 0, inputs,
        sourceDigest: sha(JSON.stringify(inputs)), lockfileSha256: sha(readFileSync(path.join(root, 'Cargo.lock'))) };
    };
    const before = observe(); observedSource = before;
    const env = nativeBuildEnvironment(process.env, false);
    const version = spawnSync('rustc', ['+' + rustVersion, '--version'], { cwd: root, env, encoding: 'utf8', windowsHide: true, timeout: 30000 });
    assert.equal(version.status, 0); assert.match(version.stdout, new RegExp('^rustc ' + rustVersion.replaceAll('.', '\\.') + ' '));
    for (const [name, args] of [['application', ['build']], ['tests', ['test', '--no-run']]]) {
      const configuration = name === 'tests' ? ['--config', 'profile.test.package.vello_cpu.opt-level=0'] : [];
      const argv = ['+' + rustVersion, ...args, ...configuration, '--locked', '--offline', '-j1', '-p', 'desigma-app', '--message-format=json'];
      const buildEnv = nativeBuildEnvironment(process.env, name === 'tests');
      const result = spawnSync('cargo', argv, { cwd: root, env: buildEnv, encoding: 'utf8', windowsHide: true, timeout: 50 * 60 * 1000, maxBuffer: 32 * 1024 * 1024 });
      const logPath = path.join(output, name + '.log'); writeFileSync(logPath, (result.stdout ?? '') + (result.stderr ?? ''));
      const check = { name, argv, profileOverrides: name === 'tests' ? { CARGO_PROFILE_TEST_DEBUG: '0' } : {},
        cargoConfigurationOverrides: name === 'tests' ? { 'profile.test.package.vello_cpu.opt-level': 0 } : {},
        exitCode: result.status, error: result.error?.code, logPath, logSha256: sha(readFileSync(logPath)) };
      checks.push(check);
      writeFileSync(path.join(output, 'build-progress.json'), JSON.stringify({ source: before, checks }, null, 2) + '\n');
      assert.equal(result.error, undefined); assert.equal(result.status, 0, 'Locked native build failed: ' + logPath);
      if (name === 'tests') check.velloResourceProfile = assertNativeTestResourceProfile(result.stdout);
      const executable = selectNativeArtifact(result.stdout, name === 'tests', path.join(root, 'target'));
      Object.assign(check, { executable, sha256: sha(readFileSync(executable)) });
      writeFileSync(path.join(output, 'build-progress.json'), JSON.stringify({ source: before, checks }, null, 2) + '\n');
    }
    assert.deepEqual(observe(), before, 'Source changed during native compilation');
    const [app, tests] = checks;
    const journeys = await qualify({ cmsRoot, fixtureRoot, binary: tests.executable, binarySha256: tests.sha256, appBinary: app.executable, appSha256: app.sha256 });
    assert.deepEqual(observe(), before, 'Source changed during native journeys');
    const receipt = { schemaVersion: 'cityos.desigma.native-build-qualification.v1', observedAt: new Date().toISOString(), source: before,
      rustc: version.stdout.trim(), checks, journeyReceipt: { path: path.join(journeys.output, 'receipt.json'), sha256: sha(readFileSync(path.join(journeys.output, 'receipt.json'))) },
      passed: journeys.passed, failed: journeys.failed, skipped: journeys.skipped, success: journeys.success,
      artifactBuildProvenanceQualified: !before.dirty && journeys.success, installedGuiQualified: false, embeddedWebViewQualified: false, activationQualified: false,
      nonClaims: ['Cargo compilation and six native journeys qualify local exact-byte behavior. Dirty source cannot qualify release provenance.', 'Physical GUI, installed WebView, environment activation and owner acceptance remain separate gates.'] };
    writeFileSync(path.join(output, 'receipt.json'), JSON.stringify(receipt, null, 2) + '\n'); return { output, receipt };
  } catch (error) {
    if (output) writeFileSync(path.join(output, 'failure.json'), JSON.stringify({
      schemaVersion: 'cityos.desigma.native-build-failure.v1', observedAt: new Date().toISOString(),
      source: observedSource, checks, success: false, error: String(error.message).slice(0, 4096),
      artifactBuildProvenanceQualified: false, installedGuiQualified: false, embeddedWebViewQualified: false, activationQualified: false,
      nonClaims: ['Failed compilation or qualification does not establish executed native test cases or a passing artifact.'],
    }, null, 2) + '\n');
    throw error;
  } finally { release(); }
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  assert.equal(process.argv.length, 4, 'Usage: qualify-native-build.mjs <CMS root> <native fixture root>');
  const { output, receipt } = await qualifyNativeBuild(...process.argv.slice(2)); console.log(JSON.stringify({ output, success: receipt.success, passed: receipt.passed, failed: receipt.failed, skipped: receipt.skipped, artifactBuildProvenanceQualified: receipt.artifactBuildProvenanceQualified })); process.exitCode = receipt.success ? 0 : 1;
}
