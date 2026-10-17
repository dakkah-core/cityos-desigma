#!/usr/bin/env node
// Inspect upgrade risk without merging, switching refs or touching the index.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, readlinkSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function sanitizeDiagnostic(value) {
  return value.replace(/(https?:\/\/)[^\s/@]+@/g, '$1<redacted>@');
}

export function parseMergeTree(output, status) {
  if (status !== 0 && status !== 1) throw new Error('Upstream integration simulation failed');
  const [tree, ...lines] = output.trimEnd().split(/\r?\n/);
  if (!/^[a-f0-9]{40,64}$/.test(tree)) throw new Error('Invalid simulated tree identity');
  const separator = lines.indexOf('');
  const conflicts = (separator === -1 ? lines : lines.slice(0, separator)).filter(Boolean);
  if (status === 0 && conflicts.length) throw new Error('Unexpected conflict records in a clean simulation');
  return { tree, clean: status === 0, conflicts };
}

export function classifyPath(path) {
  if (/^crates\/desigma-cityos(?:-catalog)?\//.test(path)) return 'cityos-package';
  if (/^crates\/desigma-app\/src\/(?:cityos_catalog\/|design_workspace\/|cityos_jobs\.rs)/.test(path)) return 'cityos-editor-adapter';
  if (/^crates\/desigma-(?:doc|library)\//.test(path)) return 'document-and-persistence';
  if (/^crates\/desigma-gpu\//.test(path)) return 'preserve-gpu';
  if (/^crates\/desigma-app\/(?:src\/plugins\/|assets\/plugins\/)/.test(path)) return 'preserve-plugins';
  if (/(?:^|\/)Cargo\.(?:toml|lock)$/.test(path)) return 'build-manifest';
  if (/^crates\/desigma-app\//.test(path)) return 'editor-hook-review';
  if (/^(?:scripts\/upstream-audit|README|docs\/|\.impeccable)/.test(path)) return 'maintenance';
  return 'review-required';
}

export function inputDigest(status, trackedDiff, untracked = []) {
  const hash = createHash('sha256');
  for (const value of [status, trackedDiff, ...untracked]) {
    const bytes = Buffer.from(value);
    hash.update(String(bytes.length)).update('\0').update(bytes);
  }
  return hash.digest('hex');
}

export function audit(repository, { fetch = false } = {}) {
  const git = (args, allowed = [0]) => {
    const result = spawnSync('git', args, { cwd: repository, encoding: 'utf8', timeout: 30_000, maxBuffer: 8 * 1024 * 1024,
      env: { ...process.env, GIT_TERMINAL_PROMPT: '0', GCM_INTERACTIVE: 'never' } });
    if (result.error || !allowed.includes(result.status)) {
      throw new Error(sanitizeDiagnostic(result.error?.message || result.stderr || `git ${args[0]} failed (${result.status})`));
    }
    return result;
  };
  const headBefore = git(['rev-parse', 'HEAD']).stdout.trim();
  const capture = () => {
    const status = git(['status', '--porcelain=v1', '--untracked-files=all', '-z']).stdout;
    const trackedDiff = git(['diff', '--binary', 'HEAD', '--']).stdout;
    const untracked = [];
    for (const record of status.split('\0').filter(row => row.startsWith('?? '))) {
      const path = record.slice(3);
      const absolute = resolve(repository, path);
      const metadata = lstatSync(absolute);
      if (metadata.size > 16 * 1024 * 1024) throw new Error('Untracked audit input exceeds its 16 MiB limit');
      untracked.push(path, metadata.isSymbolicLink() ? readlinkSync(absolute) : readFileSync(absolute));
    }
    return { status, digest: inputDigest(status, trackedDiff, untracked) };
  };
  const before = capture();
  const branch = git(['branch', '--show-current']).stdout.trim();
  if (branch !== 'main') throw new Error('Run the upgrade audit in the existing main checkout');
  // This updates only the declared upstream remote-tracking refs and object store.
  if (fetch) git(['-c', 'credential.interactive=false', 'fetch', '--no-tags', 'upstream']);
  const upstream = git(['rev-parse', '--verify', 'refs/remotes/upstream/main^{commit}']).stdout.trim();
  const baseResult = git(['merge-base', headBefore, upstream], [0, 1]);
  const base = baseResult.status === 0 ? baseResult.stdout.trim() : null;
  const counts = git(['rev-list', '--left-right', '--count', `${headBefore}...${upstream}`]).stdout.trim().split(/\s+/).map(Number);
  if (counts.length !== 2 || counts.some(value => !Number.isSafeInteger(value))) throw new Error('Invalid divergence result');
  let integration = { clean: false, conflicts: [], unavailable: 'Unrelated histories require an explicit migration decision' };
  if (base) {
    const simulation = git(['merge-tree', '--write-tree', '--name-only', '--messages', headBefore, upstream], [0, 1]);
    integration = parseMergeTree(simulation.stdout, simulation.status);
  }
  const paths = base ? git(['diff', '--name-only', '-z', `${base}..${headBefore}`]).stdout.split('\0').filter(Boolean) : [];
  const headAfter = git(['rev-parse', 'HEAD']).stdout.trim();
  const after = capture();
  if (headBefore !== headAfter || before.digest !== after.digest) throw new Error('Checkout changed during audit; repeat on stable inputs');
  return {
    schemaVersion: 'desigma.cityos.upstream-audit.v1', observedAt: new Date().toISOString(),
    head: headBefore, branch, upstream, upstreamRefreshed: fetch, mergeBase: base,
    ahead: counts[0], behind: counts[1], integration,
    dirtyInputDigest: before.digest, integrationScope: 'committed-heads',
    changedPaths: paths.map(path => ({ path, classification: classifyPath(path) })),
    checkoutUnchanged: true, nonClaims: ['No merge or acceptance performed', 'Uncommitted inputs are fingerprinted but excluded from the merge simulation', 'A clean textual simulation does not prove compatibility or passing tests'],
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2);
    if (args.some(arg => arg !== '--fetch' && !arg.startsWith('--output='))) throw new Error('Usage: node scripts/upstream-audit.mjs [--fetch] [--output=report.json]');
    const report = audit(resolve(fileURLToPath(new URL('..', import.meta.url))), { fetch: args.includes('--fetch') });
    const output = args.find(arg => arg.startsWith('--output='))?.slice('--output='.length);
    if (output) writeFileSync(resolve(output), `${JSON.stringify(report, null, 2)}\n`, { flag: 'wx' });
    console.log(JSON.stringify(report, null, 2));
  } catch (error) {
    console.error(sanitizeDiagnostic(error instanceof Error ? error.message : String(error)));
    process.exitCode = 1;
  }
}
