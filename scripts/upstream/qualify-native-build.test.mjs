import assert from 'node:assert/strict';
import test from 'node:test';
import path from 'node:path';
import { selectNativeArtifact, nativeBuildEnvironment, assertNativeTestResourceProfile } from './qualify-native-build.mjs';

test('native build selects the actual executable for the requested Cargo profile', () => {
  const target = path.resolve('target'), app = path.join(target, 'debug', 'desigma.exe'), tests = path.join(target, 'debug', 'deps', 'desigma-test.exe');
  const row = (executable, test) => JSON.stringify({ reason: 'compiler-artifact', target: { name: 'desigma', kind: ['bin'] }, profile: { test }, executable });
  const log = row(app, false) + '\n' + row(tests, true);
  assert.equal(selectNativeArtifact(log, false, target), app); assert.equal(selectNativeArtifact(log, true, target), tests);
  assert.throws(() => selectNativeArtifact('', false, target));
  assert.throws(() => selectNativeArtifact(row(path.resolve('elsewhere/desigma.exe'), false), false, target));
  assert.throws(() => selectNativeArtifact(log + '\n' + row(path.join(target, 'other.exe'), false), false, target));
});

test('native resource profile is explicit and cannot borrow ambient compiler or Cargo overrides', () => {
  assert.deepEqual(nativeBuildEnvironment({ SystemRoot: 'C:/Windows' }, false), { SystemRoot: 'C:/Windows' });
  assert.deepEqual(nativeBuildEnvironment({ SystemRoot: 'C:/Windows' }, true), { SystemRoot: 'C:/Windows', CARGO_PROFILE_TEST_DEBUG: '0' });
  for (const name of ['RUSTFLAGS', 'RUSTC_WRAPPER', 'CARGO_TARGET_DIR', 'CARGO_PROFILE_TEST_DEBUG', 'CARGO_PROFILE_DEV_OPT_LEVEL'])
    assert.throws(() => nativeBuildEnvironment({ [name]: 'foreign' }, true), /Unrecorded/);
});

test('actual Cargo artifacts must prove the bounded resource profile preserves runtime assertions', () => {
  const profile = { opt_level: '0', debuginfo: 0, debug_assertions: true, overflow_checks: true, test: false };
  const library = { reason: 'compiler-artifact', target: { name: 'vello_cpu', kind: ['lib'] }, profile };
  const tests = { reason: 'compiler-artifact', target: { name: 'desigma', kind: ['bin'] }, profile: { ...profile, test: true } };
  const log = (...rows) => rows.map(row => JSON.stringify(row)).join('\n');
  assert.deepEqual(assertNativeTestResourceProfile(log(library, tests)), profile);
  assert.throws(() => assertNativeTestResourceProfile(log(tests)), /vello_cpu compiler profile/);
  for (const patch of [{ opt_level: '2' }, { debuginfo: 2 }, { debug_assertions: false }, { overflow_checks: false }])
    assert.throws(() => assertNativeTestResourceProfile(log({ ...library, profile: { ...profile, ...patch } }, tests)));
  assert.throws(() => assertNativeTestResourceProfile(log(library, { ...tests, profile: { ...tests.profile, debug_assertions: false } })));
});
