"""Mutation controls for the offline identity gate; production pins stay untouched."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('pins', Path(__file__).with_name('verify-protocol-pins.py'))
pins = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pins)


class IdentityDrift(unittest.TestCase):
    def setUp(self):
        self.original = pins.MANIFEST
        self.manifest = json.loads(self.original.read_text())
        self.temp = tempfile.TemporaryDirectory(prefix='pablo-k01-pin-')
        pins.MANIFEST = Path(self.temp.name) / 'pins.json'

    def tearDown(self):
        pins.MANIFEST = self.original
        self.temp.cleanup()

    def verify(self):
        pins.MANIFEST.write_text(json.dumps(self.manifest))
        with contextlib.redirect_stdout(io.StringIO()):
            pins.verify()

    def test_current_identity(self):
        self.verify()

    def test_fixture_byte_drift(self):
        self.manifest['files']['tests/fixtures/a2a/card.json']['sha256'] = '0' * 64
        with self.assertRaisesRegex(AssertionError, 'File identity drift'):
            self.verify()

    def test_rust_sdk_drift(self):
        self.manifest['rust_sdks'][0]['checksum'] = '0' * 64
        with self.assertRaisesRegex(AssertionError, 'Cargo SDK drift'):
            self.verify()

    def test_node_sdk_drift(self):
        self.manifest['node_sdks']['ajv']['version'] = '0.0.0'
        with self.assertRaisesRegex(AssertionError, 'npm SDK drift'):
            self.verify()

    def test_independent_python_source_drift(self):
        self.manifest['python_peers'][0]['packages']['mcp']['source_sha256'] = '0' * 64
        with self.assertRaisesRegex(AssertionError, 'Installed Python SDK drift'):
            self.verify()


if __name__ == '__main__':
    unittest.main()
