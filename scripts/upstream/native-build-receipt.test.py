import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile
import sys

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("native_receipt", Path(__file__).with_name("native-build-receipt.py"))
owner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(owner)


class NativeReceiptTests(unittest.TestCase):
    def fixture(self, root):
        rows = []
        for name in ("desigma", "desigma-diff"):
            file = root / (name + ".exe")
            file.write_bytes(("native binary fixture " + name).encode())
            rows.append({"reason": "compiler-artifact", "target": {"name": name, "kind": ["bin"]},
                         "profile": {"test": False, "opt_level": "3"}, "executable": str(file), "fresh": False})
        return rows + [{"reason": "build-finished", "success": True}]

    def test_compiler_selects_both_exact_release_binaries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rows = self.fixture(root)
            result = owner.select_artifacts(rows, root, True)
            self.assertEqual(set(result), {"desigma", "desigma-diff"})
            self.assertEqual(result["desigma"]["sha256"], owner.digest(root / "desigma.exe"))

    def test_missing_duplicate_failed_and_foreign_compiler_results_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rows = self.fixture(root)
            for candidate in (rows[:-1], rows[1:], rows + [rows[0]], rows[:-1] + [{"reason": "build-finished", "success": False}]):
                with self.assertRaises(ValueError):
                    owner.select_artifacts(candidate, root, True)
            foreign = root / "foreign"
            foreign.mkdir()
            (foreign / "desigma.exe").write_bytes(b"another binary")
            rows[0]["executable"] = str(foreign / "desigma.exe")
            with self.assertRaises(ValueError):
                owner.select_artifacts(rows, root, True)

    def test_only_the_exact_compiled_executables_and_complete_package_qualify(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            selected = owner.select_artifacts(self.fixture(root), root, True)
            archive = root / "candidate.zip"
            asset_names = ("LICENSE-MIT", "LICENSE-APACHE", "README.md", "licenses/Inter-OFL.txt", "licenses/Lucide-LICENSE.txt")
            assets = {name: hashlib.sha256(b"license fixture").hexdigest() for name in asset_names}
            def pack(replacement=False, extra=False, changed_asset=False):
                with zipfile.ZipFile(archive, "w") as package:
                    for name in ("desigma.exe", "desigma-diff.exe"):
                        package.writestr("candidate/" + name, b"replacement" if replacement else (root / name).read_bytes())
                    for name in asset_names:
                        package.writestr("candidate/" + name, "changed license fixture" if changed_asset else "license fixture")
                    if extra:
                        package.writestr("candidate/../unexpected", "unsafe")
                Path(str(archive) + ".sha256").write_text(owner.digest(archive) + "  " + archive.name + "\n")
            pack()
            self.assertEqual(owner.qualify_zip(archive, selected, assets)["sha256"], owner.digest(archive))
            for replacement, extra, changed_asset in ((True, False, False), (False, True, False), (False, False, True)):
                pack(replacement, extra, changed_asset)
                with self.assertRaises(ValueError):
                    owner.qualify_zip(archive, selected, assets)


if __name__ == "__main__":
    unittest.main(verbosity=2)
