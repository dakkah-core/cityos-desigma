import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, readFileSync, realpathSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

export const sha256 = (bytes) =>
  createHash("sha256").update(bytes).digest("hex");
const git = (root, args) =>
  execFileSync("git", ["-C", root, ...args], {
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 16 * 1024 * 1024,
  });
const paths = (value) => value.split("\0").filter(Boolean);

export function validPath(path) {
  return (
    typeof path === "string" &&
    path.length > 0 &&
    !path.includes("\\") &&
    !path.includes(":") &&
    !path.startsWith("/") &&
    path.split("/").every((part) => part && part !== "." && part !== "..")
  );
}

export function checkedFile(root, path) {
  if (!validPath(path)) throw Error(`Invalid integration path: ${path}`);
  const file = resolve(root, path);
  if (!existsSync(file)) throw Error(`Missing integration file: ${path}`);
  const within = relative(realpathSync(root), realpathSync(file));
  if (
    within === ".." ||
    within.startsWith(`..${sep}`) ||
    isAbsolute(within) ||
    !lstatSync(file).isFile()
  ) {
    throw Error(`Integration file escapes its repository: ${path}`);
  }
  return file;
}

export function validateManifest(manifest) {
  const errors = [];
  if (manifest?.schemaVersion !== "cityos.upstream-integration.v1")
    return ["Invalid integration schema"];
  if (
    !/^https:\/\/github\.com\/[^/]+\/[^/]+(?:\.git)?$/u.test(
      manifest.upstream?.repository ?? "",
    )
  )
    errors.push("Invalid upstream repository");
  if (!/^[a-f0-9]{40}$/u.test(manifest.upstream?.commit ?? ""))
    errors.push("Upstream must use a full commit SHA");
  if (!manifest.upstream?.branch) errors.push("Missing upstream branch");
  const declared = new Set();
  for (const change of manifest.changes ?? []) {
    if (
      !change.id ||
      !change.purpose ||
      !change.invariant ||
      !change.tests?.length ||
      !change.paths?.length
    )
      errors.push(`Incomplete change: ${change.id}`);
    for (const path of [...(change.paths ?? []), ...(change.tests ?? [])]) {
      if (!validPath(path)) errors.push(`Invalid change path: ${path}`);
    }
    for (const path of change.paths ?? []) {
      if (declared.has(path)) errors.push(`Duplicate change path: ${path}`);
      declared.add(path);
    }
  }
  for (const path of manifest.ownedDirectories ?? [])
    if (!validPath(path)) errors.push(`Invalid owned directory: ${path}`);
  return errors;
}

export function verifyDeclaredChanges(manifest, changedPaths) {
  const errors = validateManifest(manifest);
  const declared = new Set(
    (manifest.changes ?? []).flatMap((change) => change.paths ?? []),
  );
  for (const path of changedPaths) {
    const owned = (manifest.ownedDirectories ?? []).some(
      (directory) => path === directory || path.startsWith(directory + "/"),
    );
    if (!owned && !declared.has(path))
      errors.push(`Undeclared upstream modification: ${path}`);
  }
  return errors;
}

export function captureSourceIdentity(root) {
  const head = git(root, ["rev-parse", "HEAD"]).trim();
  const status = git(root, [
    "status",
    "--porcelain=v1",
    "-z",
    "--untracked-files=all",
  ]);
  const names = paths(
    git(root, ["ls-files", "--cached", "--others", "--exclude-standard", "-z"]),
  );
  const hash = createHash("sha256");
  for (const name of [...new Set(names)].sort()) {
    hash.update(name).update("\0");
    if (!existsSync(resolve(root, name))) {
      hash.update("deleted\0");
      continue;
    }
    hash.update(readFileSync(checkedFile(root, name))).update("\0");
  }
  return {
    sourceSha: head,
    sourceDigest: hash.digest("hex"),
    dirty: status.length !== 0,
  };
}

export function verifyIntegration(root, { requireIntegrated = true } = {}) {
  const manifest = JSON.parse(
    readFileSync(resolve(root, "config/upstream/integration.json"), "utf8"),
  );
  const errors = validateManifest(manifest);
  if (errors.length) return { errors };
  let customized;
  try {
    customized = paths(
      git(root, [
        "diff",
        "--name-only",
        "-z",
        `${manifest.upstream.commit}...HEAD`,
      ]),
    );
    customized.push(...paths(git(root, ["diff", "--name-only", "-z", "HEAD"])));
    customized.push(
      ...paths(git(root, ["ls-files", "--others", "--exclude-standard", "-z"])),
    );
    if (requireIntegrated) {
      try {
        git(root, [
          "merge-base",
          "--is-ancestor",
          manifest.upstream.commit,
          "HEAD",
        ]);
      } catch {
        errors.push("Selected upstream commit is not integrated into HEAD");
      }
    }
  } catch {
    return {
      errors: [
        "Pinned upstream commit is unavailable; fetch the declared upstream before qualification",
      ],
    };
  }
  errors.push(...verifyDeclaredChanges(manifest, [...new Set(customized)]));
  for (const path of new Set(
    manifest.changes.flatMap((change) => change.tests),
  )) {
    try {
      checkedFile(root, path);
    } catch (error) {
      errors.push(error.message);
    }
  }
  return {
    schemaVersion: "cityos.upstream-source-check.v1",
    ...captureSourceIdentity(root),
    upstream: manifest.upstream,
    changedPaths: [...new Set(customized)].sort(),
    errors,
  };
}

export function verifyReceipt(
  root,
  receipt,
  identity = captureSourceIdentity(root),
) {
  const errors = [];
  if (receipt?.schemaVersion !== "cityos.upstream-qualification.v1")
    return ["Invalid qualification receipt"];
  if (identity.dirty || receipt.dirty !== false)
    errors.push("Qualification requires a clean recorded source tree");
  if (
    receipt.sourceSha !== identity.sourceSha ||
    receipt.sourceDigest !== identity.sourceDigest
  )
    errors.push("Qualification source identity differs");
  const manifest = JSON.parse(
    readFileSync(checkedFile(root, "config/upstream/integration.json"), "utf8"),
  );
  const required = new Set(manifest.requiredChecks ?? []);
  if (
    JSON.stringify([...(receipt.requiredChecks ?? [])].sort()) !==
    JSON.stringify([...required].sort())
  )
    errors.push("Receipt required checks differ from the integration manifest");
  if (!required.size) errors.push("Qualification has no required checks");
  for (const name of required) {
    const matches = (receipt.checks ?? []).filter(
      (check) => check.name === name,
    );
    if (
      matches.length !== 1 ||
      matches[0].exitCode !== 0 ||
      matches[0].passed < 1 ||
      matches[0].failed !== 0 ||
      matches[0].skipped !== 0
    )
      errors.push(`Required check did not pass: ${name}`);
    else {
      try {
        if (
          sha256(readFileSync(checkedFile(root, matches[0].logPath))) !==
          matches[0].logSha256
        )
          errors.push(`Check log differs: ${name}`);
      } catch (error) {
        errors.push(error.message);
      }
    }
  }
  try {
    if (
      sha256(readFileSync(checkedFile(root, receipt.artifact.path))) !==
      receipt.artifact.sha256
    )
      errors.push("Qualified artifact bytes differ");
    if (
      sha256(readFileSync(checkedFile(root, receipt.lockfile.path))) !==
      receipt.lockfile.sha256
    )
      errors.push("Qualified lockfile bytes differ");
  } catch (error) {
    errors.push(error.message);
  }
  return errors;
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const args = process.argv.slice(2);
  const options = [...args];
  if (options[0] === "--inventory") options.shift();
  if (
    options.length &&
    !(
      options.length === 2 &&
      options[0] === "--receipt" &&
      validPath(options[1])
    )
  )
    throw Error("Usage: integration.mjs [--inventory] [--receipt path]");
  const root = fileURLToPath(new URL("../../", import.meta.url));
  const report = verifyIntegration(root, {
    requireIntegrated: !args.includes("--inventory"),
  });
  if (args.includes("--receipt"))
    report.errors.push(
      ...verifyReceipt(
        root,
        JSON.parse(
          readFileSync(
            checkedFile(root, args[args.indexOf("--receipt") + 1]),
            "utf8",
          ),
        ),
      ),
    );
  console.log(JSON.stringify(report, null, 2));
  process.exitCode = report.errors.length ? 1 : 0;
}
