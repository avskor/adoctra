"""Pinned upstream selection, traceability, and rejection of stale test locations."""
import copy
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import acceptance
from compatibility import CORPUS, case_commands, validate_attributes


class UpstreamTests(unittest.TestCase):
    def setUp(self):
        self.path = CORPUS / 'upstream/cases.json'
        self.manifest, self.cases = acceptance.load_corpus(self.path)

    def test_selection_has_pinned_tests_resources_license_and_explicit_adapters(self):
        self.assertEqual(len(self.cases), 43)
        self.assertEqual({c['category'] for c in self.cases}, {'attributes', 'substitutions', 'include'})
        for origin in self.manifest['origins'].values():
            self.assertRegex(origin['revision'], r'^[0-9a-f]{40}$')
            self.assertTrue(origin['source'].endswith('/tree/' + origin['revision']))
            self.assertIn(origin['notice']['path'], self.manifest['files'])
        for case in self.manifest['cases']:
            upstream = case['upstream']
            self.assertTrue(self.manifest['files'][upstream['file']]['upstream_path'].startswith('test/'))
            self.assertGreater(len(upstream['adaptation']), 30)
            for resource in case.get('resources', {}).values():
                self.assertTrue(self.manifest['files'][resource]['upstream_path'].startswith('test/fixtures/'))

    def test_stale_or_missing_upstream_locations_are_rejected(self):
        original_read = Path.read_text
        for change in [{'line': 0}, {'line': 100000}, {'line': True}, {'line': '305'}, {'test': 'wrong name'}, {'adaptation': ''}]:
            manifest = copy.deepcopy(self.manifest)
            manifest['cases'][0]['upstream'].update(change)

            def read(path, *args, **kwargs):
                return json.dumps(manifest) if path == self.path else original_read(path, *args, **kwargs)

            with self.subTest(change=change), patch.object(Path, 'read_text', read):
                with self.assertRaisesRegex(ValueError, 'upstream'):
                    acceptance.load_corpus(self.path)

    def test_string_override_markers_are_forwarded_without_reinterpretation(self):
        for attrs in [{'cash@': 'heroes'}, {'cash': 'heroes@'}, {'!cash': ''}, {'cash!': '@'}, {'!cash@': ''}, {'cash!@': ''}]:
            case = {'id': 'host', 'source': '', 'attributes': attrs}
            with case_commands(case, Path('/tmp/adoctra')) as (ruby, rust):
                arg = next(k + '=' + v for k, v in attrs.items())
                self.assertIn(arg, ruby)
                self.assertIn(arg, rust)
        for name in ['!cash!', '@cash', 'cash@@', '!@', 'cash@!', 'cash=x']:
            with self.subTest(name=name), self.assertRaises(ValueError):
                validate_attributes({'attributes': {name: ''}})


if __name__ == '__main__':
    unittest.main()
