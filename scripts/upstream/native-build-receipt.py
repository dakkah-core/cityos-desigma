"""Bind canonical native packages to Cargo's actual selected executables."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tomllib
import zipfile
from datetime import datetime, timezone


def digest(file):
    value = hashlib.sha256()
    with open(file, "rb") as stream:
        for block in iter(lambda: stream.read(512 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def select_artifacts(messages, release_root, windows):
    release_root = Path(release_root).resolve(strict=True)
    selected, finished = {}, False
    for row in messages:
        if row.get("reason") == "build-finished":
            if finished or row.get("success") is not True:
                raise ValueError("native build did not finish successfully exactly once")
            finished = True
        if row.get("reason") != "compiler-artifact" or row.get("target", {}).get("name") not in ("desigma", "desigma-diff"):
            continue
        target = row["target"]
        if target.get("kind") != ["bin"] or row.get("profile", {}).get("test") is not False:
            continue
        name = target["name"]
        if name in selected or not row.get("executable"):
            raise ValueError("missing or duplicate compiler-selected native binary")
        original = Path(row["executable"])
        file = original.resolve(strict=True)
        if original.is_symlink() or not file.is_file() or file.parent != release_root or file.name != name + (".exe" if windows else ""):
            raise ValueError("compiler-selected binary escaped the exact target release directory")
        selected[name] = {"path": str(file), "sha256": digest(file), "bytes": file.stat().st_size,
                          "profile": row["profile"], "fresh": row.get("fresh")}
    if not finished or set(selected) != {"desigma", "desigma-diff"}:
        raise ValueError("complete Cargo build and both actual binaries are required")
    return selected


def qualify_zip(archive, selected, assets=None):
    archive = Path(archive)
    if archive.is_symlink() or not archive.is_file():
        raise ValueError("ordinary immutable package required")
    expected = {"desigma.exe", "desigma-diff.exe", "LICENSE-MIT", "LICENSE-APACHE", "README.md",
                "licenses/Inter-OFL.txt", "licenses/Lucide-LICENSE.txt"}
    found, root = set(), None
    with zipfile.ZipFile(archive) as package:
        for entry in package.infolist():
            parts = entry.filename.split("/")
            if not parts[0] or any(part in (".", "..") for part in parts) or "\\" in entry.filename or ":" in entry.filename:
                raise ValueError("package entry escaped its root")
            root = root or parts[0]
            if parts[0] != root or root != archive.stem or stat.S_ISLNK(entry.external_attr >> 16):
                raise ValueError("package root or entry type changed")
            if entry.is_dir():
                continue
            ref = "/".join(parts[1:])
            if ref not in expected or ref in found or entry.file_size > 512 * 1024 * 1024:
                raise ValueError("missing, duplicate or unexpected package payload")
            found.add(ref)
            if ref in ("desigma.exe", "desigma-diff.exe") or assets is not None:
                value = hashlib.sha256()
                with package.open(entry) as stream:
                    for block in iter(lambda: stream.read(512 * 1024), b""):
                        value.update(block)
                expected_digest = selected[ref[:-4]]["sha256"] if ref.endswith(".exe") else assets[ref]
                if value.hexdigest() != expected_digest:
                    raise ValueError("packaged payload differs from its source or compiler-selected binary")
    if found != expected:
        raise ValueError("canonical Windows package is incomplete")
    result = {"path": str(archive.resolve()), "sha256": digest(archive), "bytes": archive.stat().st_size}
    checksum = Path(str(archive) + ".sha256").read_text().split()
    if len(checksum) != 2 or checksum != [result["sha256"], archive.name]:
        raise ValueError("canonical package checksum differs")
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--archive")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    def git(*argv):
        return subprocess.check_output(["git", "--no-optional-locks", "-C", str(root), *argv], text=True).strip()
    source = git("rev-parse", "HEAD")
    if git("status", "--porcelain=v1", "--untracked-files=all") or source != os.environ.get("GITHUB_SHA"):
        raise ValueError("exact clean CI source required")
    canonical = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    rustc = subprocess.check_output(["rustc", "--version"], text=True).strip()
    if not rustc.startswith("rustc " + canonical + " ") or os.environ.get("CARGO_BUILD_JOBS") != "1":
        raise ValueError("canonical Rust and the recorded serial build are required")
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version=1", "--no-deps"], cwd=root))
    target_root = Path(metadata["target_directory"]) / args.target / "release"
    if not target_root.resolve(strict=True).is_relative_to(Path(metadata["target_directory"]).resolve(strict=True)):
        raise ValueError("native target escaped the compiler output directory")
    with open(args.log, encoding="utf-8-sig") as stream:
        selected = select_artifacts((json.loads(line) for line in stream if line.strip()), target_root, "windows" in args.target)
    asset_paths = {"LICENSE-MIT": "LICENSE-MIT", "LICENSE-APACHE": "LICENSE-APACHE", "README.md": "README.md",
                   "licenses/Inter-OFL.txt": "crates/desigma-fonts/assets/inter/OFL.txt",
                   "licenses/Lucide-LICENSE.txt": "crates/desigma-app/assets/icons/lucide/LICENSE"}
    assets = {name: digest(root / ref) for name, ref in asset_paths.items()}
    archive = qualify_zip(args.archive, selected, assets) if args.archive else None
    if git("rev-parse", "HEAD") != source or git("status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("native build source changed")
    receipt = {"schemaVersion": "cityos.desigma.compiler-package.v1", "sourceCommit": source,
               "recordedAt": datetime.now(timezone.utc).isoformat(),
               "lockfileSha256": digest(root / "Cargo.lock"), "rustc": rustc, "target": args.target,
               "resources": {"cargoBuildJobs": 1}, "compilerLogSha256": digest(args.log),
               "binaries": selected, "package": archive, "assetDigests": assets, "cleanSourceBuildQualified": True,
               "packagedBytesQualified": archive is not None, "installedGuiQualified": False,
               "embeddedWebViewQualified": False, "nativeJourneysQualified": False, "activationQualified": False,
               "nonClaims": ["Compiler/package identity does not qualify installed GUI, WebView, persistence recovery or activation."]}
    Path(args.output).write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"receipt": args.output, "sourceCommit": source, "packagedBytesQualified": archive is not None}))


if __name__ == "__main__":
    main()
