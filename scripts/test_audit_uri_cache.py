import copy
import subprocess
from contextlib import nullcontext
from pathlib import Path
import unittest
from unittest.mock import patch

import audit_uri_cache as audit


class UriCacheAuditTests(unittest.TestCase):
    def setUp(self):
        self.case = {'requests': ['/part.adoc']}
        self.report = {'html': '<p>one</p>', 'diagnostics': [],
                       'requests': [{'path': '/part.adoc', 'status': 200, 'sha256': 'abc'}]}

    def test_extra_fetch_and_wrong_response_are_differences(self):
        changed = copy.deepcopy(self.report)
        changed['requests'] *= 2
        self.assertEqual(audit.compare(self.case, self.report, changed), 'DIFF')
        changed['requests'] = [dict(self.report['requests'][0], sha256='def')]
        self.assertEqual(audit.compare(self.case, self.report, changed), 'DIFF')

    def test_ruby_exceptions_are_not_pass_or_hidden_request_failures(self):
        reference = dict(self.report, error={'class': 'EncodingError'})
        self.assertEqual(audit.compare(self.case, reference, self.report), 'UNEXPECTED_RUBY_ERROR')
        self.case['ruby_error'] = 'EncodingError'
        self.assertEqual(audit.compare(self.case, reference, self.report), 'RUBY_ERROR')
        self.assertEqual(audit.compare(self.case, self.report, self.report), 'UNEXPECTED_RUBY_SUCCESS')
        self.assertEqual(audit.compare(self.case, reference, dict(self.report, requests=[])), 'DIFF')
        self.assertEqual(audit.compare(self.case, dict(reference, error={'class': 'OtherError'}), self.report), 'UNEXPECTED_RUBY_ERROR')

    def test_cli_failure_cannot_be_quarantined_as_a_ruby_exception(self):
        case = dict(source='text', responses={})
        with patch.object(audit, 'server', return_value=nullcontext(('http://fixture.test', []))), \
                patch.object(audit.subprocess, 'run',
                             return_value=subprocess.CompletedProcess([], 1, b'', b'failed')):
            with self.assertRaisesRegex(RuntimeError, 'failed'):
                audit.run_side(case, Path('/tmp/cli'), False)

    def test_lone_carriage_return_of_the_cli_survives(self):
        # Output is read as bytes: text mode would turn a lone CR into LF.
        done = subprocess.CompletedProcess([], 0, b'<pre>a\rb</pre>', b'')
        with patch.object(audit, 'server', return_value=nullcontext(('http://fixture.test', []))), \
                patch.object(audit.subprocess, 'run', return_value=done) as run:
            result = audit.run_side(dict(source='text', responses={}), Path('/tmp/cli'), False)
        self.assertEqual(result['html'], '<pre>a\rb</pre>')
        self.assertIsInstance(run.call_args.kwargs['input'], bytes)
        self.assertNotIn('text', run.call_args.kwargs)

    def test_positions_and_literal_newlines_are_not_normalized_away(self):
        expected = dict(self.report, html='<pre>one\ntwo</pre>')
        self.assertEqual(audit.compare(self.case, expected, dict(self.report, html='<pre>one two</pre>')), 'DIFF')
        diagnostic = dict(severity='error', message='error', location=dict(file='main.adoc', line=1))
        expected = dict(self.report, diagnostics=[diagnostic])
        actual = dict(self.report, diagnostics=[dict(diagnostic, location=dict(file='main.adoc', line=2))])
        self.assertEqual(audit.compare(self.case, expected, actual), 'DIFF')

    def test_uri_diagnostic_cursor_and_transport_names(self):
        result = audit.cli_diagnostics('adoctra: http://127.0.0.1:123/a.adoc:2: error: message\n')
        self.assertEqual(result[0]['location'], dict(file='http://127.0.0.1:123/a.adoc', line=2))
        self.assertEqual(audit.canonical(result, '/tmp/fixture', 'http://127.0.0.1:123')[0]['location']['file'],
                         'http://fixture.test/a.adoc')
        with self.assertRaises(ValueError):
            audit.cli_diagnostics('unexpected output')

    def test_unique_cases_have_explicit_request_expectations(self):
        cases = audit.cases()
        self.assertEqual(len(cases), len({case['id'] for case in cases}))
        self.assertTrue(all(isinstance(case['requests'], list) for case in cases))
        self.assertIn('unicode-hit-ruby-error', {case['id'] for case in cases if case.get('ruby_error')})

    def test_styles_and_links_compare_exactly_and_shell_mapping_is_narrow(self):
        self.case['standalone'] = True
        reference = dict(self.report, html='<div id="content"><p>one</p></div>', styles=['a\nb'], stylesheet_links=['<link rel="stylesheet" href="a.css">'])
        actual = dict(reference, html='<main id="content"><p>one</p></main>')
        self.assertEqual(audit.compare(self.case, reference, actual), 'PASS')
        self.assertEqual(audit.compare(self.case, reference, dict(actual, styles=['a b'])), 'DIFF')
        self.assertEqual(audit.compare(self.case, reference, dict(actual, stylesheet_links=[])), 'DIFF')
        self.assertEqual(audit.compare(self.case, reference, dict(actual, html='<main id="content" role="extra"><p>one</p></main>')), 'DIFF')

    def test_known_order_gap_is_not_pass_and_unexpected_success_fails(self):
        case = dict(self.case, known_difference='order')
        self.assertEqual(audit.compare(case, self.report, self.report), 'XPASS')
        self.assertEqual(audit.compare(case, self.report, dict(self.report, requests=[])), 'XFAIL')
        self.assertEqual(len(audit.asset_cases()), len({c['id'] for c in audit.asset_cases()}))

    def test_asset_warnings_retain_absent_cursors(self):
        result = audit.cli_diagnostics('adoctra: warning: could not retrieve image data from URI: http://fixture.test/a.png\n')
        self.assertEqual(result[0]['location'], None)
        self.assertEqual(result[0]['severity'], 'warning')


if __name__ == '__main__':
    unittest.main()
