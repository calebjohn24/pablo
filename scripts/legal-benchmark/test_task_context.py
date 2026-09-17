import tempfile
import unittest
from pathlib import Path

from task_context import task_context


class TaskContextTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'task.md').write_text('Apply the supplied rules.')
        (self.root / 'sources').mkdir()
        (self.root / 'sources/rule.txt').write_text('The deadline is ten days.')
        (self.root / 'key.json').write_text('PRIVATE SCORER ANSWER')

    def test_includes_actual_source_but_not_scorer(self):
        text, metadata = task_context(self.root)
        self.assertIn('The deadline is ten days.', text)
        self.assertIn('Apply the supplied rules.', text)
        self.assertNotIn('PRIVATE SCORER ANSWER', text)
        self.assertEqual(metadata['source_files'], ['sources/rule.txt'])

    def test_brief_only_mode_does_not_read_sources(self):
        (self.root / 'sources/rule.txt').unlink()
        (self.root / 'sources/rule.txt').symlink_to(self.root / 'key.json')
        text, metadata = task_context(self.root, include_sources=False)
        self.assertNotIn('PRIVATE SCORER ANSWER', text)
        self.assertFalse(metadata['sources_included'])

    def test_rejects_link_to_scorer(self):
        (self.root / 'sources/rule.txt').unlink()
        (self.root / 'sources/rule.txt').symlink_to(self.root / 'key.json')
        with self.assertRaises(ValueError):
            task_context(self.root)

    def test_rejects_non_source_files(self):
        (self.root / 'sources/key.json').write_text('PRIVATE SCORER ANSWER')
        with self.assertRaises(ValueError):
            task_context(self.root)

    def test_rejects_oversized_context_without_truncating_rules(self):
        with self.assertRaisesRegex(ValueError, 'exceeds byte limit'):
            task_context(self.root, max_bytes=40)


if __name__ == '__main__':
    unittest.main()
