"""Provenance rejection tests using isolated, local Git checkouts."""

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "import_languages", Path(__file__).with_name("import-languages.py")
)
IMPORTER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(IMPORTER)


class SourceProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.source = Path(self.temporary.name)
        self.git("init", "--quiet")
        (self.source / "dictionary.dic").write_text("1\noriginal\n")
        self.git("add", "dictionary.dic")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "commit", "--quiet", "-m", "fixture")
        self.revision = self.git("rev-parse", "HEAD")
        self.pin = patch.object(IMPORTER, "REVISION", self.revision)
        self.pin.start()
        self.addCleanup(self.pin.stop)

    def git(self, *arguments):
        return subprocess.run(
            ["git", "-C", str(self.source), *arguments], check=True,
            capture_output=True, text=True,
        ).stdout.strip()

    def test_clean_exact_revision_is_verified(self):
        self.assertEqual(IMPORTER.verify_source(self.source), self.revision)

    def test_different_revision_is_rejected(self):
        with patch.object(IMPORTER, "REVISION", "0" * 40):
            with self.assertRaisesRegex(RuntimeError, "revision must be"):
                IMPORTER.verify_source(self.source)

    def test_modified_staged_deleted_and_untracked_sources_are_rejected(self):
        tracked = self.source / "dictionary.dic"
        tracked.write_text("1\nmodified\n")
        with self.assertRaisesRegex(RuntimeError, "modified or untracked"):
            IMPORTER.verify_source(self.source)
        self.git("add", "dictionary.dic")
        with self.assertRaisesRegex(RuntimeError, "modified or untracked"):
            IMPORTER.verify_source(self.source)
        self.git("reset", "--hard", "HEAD")
        tracked.unlink()
        with self.assertRaisesRegex(RuntimeError, "modified or untracked"):
            IMPORTER.verify_source(self.source)
        self.git("reset", "--hard", "HEAD")
        (self.source / "untracked.aff").write_text("SET UTF-8\n")
        with self.assertRaisesRegex(RuntimeError, "modified or untracked"):
            IMPORTER.verify_source(self.source)

    def test_nested_directory_is_not_misattributed_to_parent_checkout(self):
        nested = self.source / "nested"
        nested.mkdir()
        with self.assertRaisesRegex(RuntimeError, "checkout root"):
            IMPORTER.verify_source(nested)

    def test_non_checkout_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "readable Git checkout"):
                IMPORTER.verify_source(Path(directory))

    def test_invalid_source_stops_before_keyboard_compilation_or_asset_writes(self):
        arguments = ["import-languages.py", str(self.source)]
        for flag in ["--library", "--xkb", "--xkb-license", "--xkb-authors",
                     "--compose", "--compose-license"]:
            arguments.extend([flag, "unused"])
        with patch.object(IMPORTER, "REVISION", "0" * 40), \
             patch("sys.argv", arguments), \
             patch.object(IMPORTER, "keyboard_tables") as compile_tables, \
             patch.object(IMPORTER.shutil, "copyfile") as copy_asset:
            with self.assertRaisesRegex(RuntimeError, "revision must be"):
                IMPORTER.main()
        compile_tables.assert_not_called()
        copy_asset.assert_not_called()


if __name__ == "__main__":
    unittest.main()
