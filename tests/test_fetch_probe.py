import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

from scripts.fetch_probe import PREFIX, extract_assets


def archive(entries):
    result = io.BytesIO()
    with tarfile.open(fileobj=result, mode="w:gz") as tar:
        for name, data, kind in entries:
            info = tarfile.TarInfo(PREFIX + name)
            info.type = kind
            info.size = len(data) if kind == tarfile.REGTYPE else 0
            tar.addfile(info, io.BytesIO(data) if kind == tarfile.REGTYPE else None)
    result.seek(0)
    return result


class FetchProbeTests(unittest.TestCase):
    def setUp(self):
        self.data = b"real asset bytes"
        self.assets = {"asset": ("safe.bin", len(self.data), hashlib.sha256(self.data).hexdigest())}
        self.temp = tempfile.TemporaryDirectory()
        self.dest = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)

    def test_valid_only_exact_whitelisted_member_is_installed(self):
        extract_assets(archive([
            ("../escape", b"bad", tarfile.REGTYPE),
            ("asset", self.data, tarfile.REGTYPE),
        ]), self.dest, self.assets)
        self.assertEqual((self.dest / "safe.bin").read_bytes(), self.data)
        self.assertFalse((self.dest.parent / "escape").exists())

    def test_wrong_hash_or_size_does_not_overwrite_existing_file(self):
        (self.dest / "safe.bin").write_bytes(b"previous")
        with self.assertRaises(ValueError):
            extract_assets(archive([("asset", b"wrong", tarfile.REGTYPE)]), self.dest, self.assets)
        self.assertEqual((self.dest / "safe.bin").read_bytes(), b"previous")
        wrong_hash = {"asset": ("safe.bin", len(self.data), "0" * 64)}
        with self.assertRaises(ValueError):
            extract_assets(archive([("asset", self.data, tarfile.REGTYPE)]), self.dest, wrong_hash)
        self.assertEqual((self.dest / "safe.bin").read_bytes(), b"previous")

    def test_missing_duplicate_and_symlink_are_rejected(self):
        cases = [
            [],
            [("asset", self.data, tarfile.REGTYPE), ("asset", self.data, tarfile.REGTYPE)],
            [("asset", b"", tarfile.SYMTYPE)],
        ]
        for entries in cases:
            with self.subTest(entries=entries), self.assertRaises(ValueError):
                extract_assets(archive(entries), self.dest, self.assets)
        self.assertFalse((self.dest / "safe.bin").exists())

    def test_destination_traversal_is_rejected(self):
        assets = {"asset": ("../escape", len(self.data), hashlib.sha256(self.data).hexdigest())}
        with self.assertRaises(ValueError):
            extract_assets(archive([("asset", self.data, tarfile.REGTYPE)]), self.dest, assets)


if __name__ == "__main__":
    unittest.main()
