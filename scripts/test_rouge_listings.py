import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import rouge_listings as rl


LISTING = '<pre class="rouge highlight"><code data-lang="java"><span class="kt">int</span> a;</code></pre>'


class RougeListingsTests(unittest.TestCase):
    def test_listings_keep_the_code_content_without_the_pre_attributes(self):
        html = ('<p>x</p>\n' + LISTING.replace('highlight"', 'highlight" id="ex"') + '\n'
                + '<pre class="highlight"><code>plain</code></pre>\n'
                + '<pre class="rouge highlight nowrap" style="color: #f8f8f2"><code>b</code></pre>')
        self.assertEqual(rl.listings(html), ['<code data-lang="java"><span class="kt">int</span> a;', '<code>b'])
        self.assertEqual(rl.listings('<p>none</p>'), [])
        with self.assertRaisesRegex(ValueError, 'unterminated'):
            rl.listings('<pre class="rouge highlight"><code>open')

    def test_configurations_add_rouge_after_the_documents_attributes(self):
        case = {'id': 'c', 'source': 'x', 'attributes': {'source-highlighter': 'highlightjs', 'icons': 'font'}}
        configured = rl.configured(case, 'linenums')
        self.assertEqual(configured['attributes'],
                         {'icons': 'font', 'source-highlighter': 'rouge', 'source-linenums-option': ''})
        self.assertEqual(case['attributes']['source-highlighter'], 'highlightjs')
        self.assertNotEqual(rl.inputs_digest(configured), rl.inputs_digest(rl.configured(case, 'default')))

    def test_stale_missing_count_and_content_differences_fail(self):
        digest = 'd'
        reference = {'c': {'inputs': digest, 'listings': ['<code>a', '<code>b']}}
        self.assertEqual(rl.compare('c', digest, reference, ['<code>a', '<code>b']), ('PASS', []))
        self.assertEqual(rl.compare('c', 'other', reference, ['<code>a', '<code>b'])[0], 'STALE')
        self.assertEqual(rl.compare('missing', digest, reference, [])[0], 'STALE')
        status, detail = rl.compare('c', digest, reference, ['<code>a'])
        self.assertEqual((status, detail), ('DIFF', ['2 listings in Ruby, 1 in adoctra']))
        status, detail = rl.compare('c', digest, reference, ['<code>a', '<code>B'])
        self.assertEqual(status, 'DIFF')
        self.assertIn('--- ruby listing 2\n', detail)
        self.assertIn('+<code>B', detail)

    def test_reference_must_match_profile_configuration_and_versions(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(rl, 'REFERENCE_DIR', Path(directory)):
            with self.assertRaisesRegex(ValueError, 'missing reference'):
                rl.load_reference('style')
            data = {'profile': rl.PROFILE, **rl.versions(), 'configuration': rl.CONFIGURATIONS['style'],
                    'cases': {'c': {'inputs': 'd', 'listings': []}}}
            path = Path(directory) / 'style.json'
            path.write_text(json.dumps(data), encoding='utf-8')
            self.assertEqual(rl.load_reference('style'), data['cases'])
            path.write_text(json.dumps(dict(data, rouge='0.0.0')), encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'mismatch'):
                rl.load_reference('style')
            path.write_text(json.dumps(dict(data, configuration={})), encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'mismatch'):
                rl.load_reference('style')

    def test_a_failing_converter_is_an_error_not_an_empty_result(self):
        failed = subprocess.CompletedProcess([], 1, b'', b'boom')
        with patch.object(rl.subprocess, 'run', return_value=failed):
            with self.assertRaisesRegex(RuntimeError, 'boom'):
                rl.run(['ruby'], 'x')
        done = subprocess.CompletedProcess([], 0, LISTING.encode(), b'warning')
        with patch.object(rl.subprocess, 'run', return_value=done):
            self.assertEqual(rl.run(['adoctra'], 'x'), [rl.listings(LISTING)[0]])

    def test_verification_skips_ruby_for_cases_already_verified(self):
        _, cases = rl.acceptance.load_corpus(rl.MANIFEST)
        case = cases[0]
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(rl, 'REFERENCE_CACHE', Path(directory) / 'cache.json'), \
                patch.object(rl, 'ruby_environment', return_value='env'), \
                patch.object(rl, 'convert', side_effect=lambda c, binary, ruby: rl.load_reference('default')[c['id']]['listings']) as convert:
            argv = ['--verify-reference', '--configuration', 'default', '--case', case['id']]
            self.assertEqual(rl.main(argv), 0)
            self.assertEqual(convert.call_count, 1)
            self.assertEqual(rl.main(argv), 0)
            self.assertEqual(convert.call_count, 1)  # the second run hits the cache
            self.assertEqual(rl.main([*argv, '--no-reference-cache']), 0)
            self.assertEqual(convert.call_count, 2)
            with patch.object(rl, 'ruby_environment', return_value='other'):
                self.assertEqual(rl.main(argv), 0)
            self.assertEqual(convert.call_count, 3)  # another Ruby environment runs Ruby again

    def test_stored_references_cover_every_configuration_and_case(self):
        _, cases = rl.acceptance.load_corpus(rl.MANIFEST)
        for configuration in rl.CONFIGURATIONS:
            reference = rl.load_reference(configuration)
            self.assertEqual(sorted(reference), sorted(case['id'] for case in cases), configuration)
            for case in cases:
                configured = rl.configured(case, configuration)
                self.assertEqual(reference[case['id']]['inputs'], rl.inputs_digest(configured),
                                 f"{configuration} {case['id']}")


if __name__ == '__main__':
    unittest.main()
