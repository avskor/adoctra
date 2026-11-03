import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import audit_include_order as audit


class IncludeOrderAuditTests(unittest.TestCase):
    def test_nul_content_difference_is_not_normalized_away(self):
        reference = subprocess.CompletedProcess([], 0, json.dumps(dict(
            html='<p>\0<strong>\0t\0w\0o\0</strong></p>', diagnostics=[])), '')
        actual = subprocess.CompletedProcess([], 0, '<p><strong>two</strong></p>', '')
        self.assertEqual(audit.compare(reference, actual, Path('/fixture'))['status'], 'DIFF')

    def test_output_is_read_as_bytes_so_a_lone_cr_survives(self):
        done = subprocess.CompletedProcess([], 0, b'<pre>a\rb</pre>', b'')
        with patch.object(audit.subprocess, 'run', return_value=done) as run:
            result = audit.invoke(['adoctra'], 'source')
        self.assertEqual(result.stdout, '<pre>a\rb</pre>')
        self.assertEqual(run.call_args.kwargs['input'], b'source')
        self.assertNotIn('text', run.call_args.kwargs)

    def test_ruby_exception_is_quarantined_and_never_a_pass(self):
        reference = subprocess.CompletedProcess([], 1, '', '/fixture/main.adoc: invalid Unicode')
        actual = subprocess.CompletedProcess([], 0, '<p>partial</p>', '')
        result = audit.compare(reference, actual, Path('/fixture'))
        self.assertEqual(result['status'], 'RUBY_ERROR')
        self.assertIn('<fixture>/main.adoc', result['reference_error'])

    def test_adoctra_failure_cannot_hide_behind_a_ruby_exception(self):
        failed = subprocess.CompletedProcess([], 1, '', 'crash')
        with self.assertRaisesRegex(RuntimeError, 'adoctra failed'):
            audit.compare(failed, failed, Path('/fixture'))

    def test_framed_probe_really_splits_inside_a_unicode_character(self):
        case = next(case for case in audit.bom_tag_cases()
                    if case['id'] == 'bom-framed-default-be-lines=1;2')
        data = bytes.fromhex(case['binary_resources']['part.adoc'])
        self.assertEqual(data.count(b'\n'), 1)
        self.assertNotIn('\n', data.decode('utf-16'))
        self.assertIn('上', data.decode('utf-16'))

    def test_later_bom_probe_has_distinct_initial_and_selected_endianness(self):
        case = next(case for case in audit.bom_tag_cases()
                    if case['id'] == 'bom-later-hint-default-le')
        data = bytes.fromhex(case['binary_resources']['part.adoc'])
        self.assertTrue(data.startswith(b'\xfe\xff'))
        self.assertTrue(data.split(b'\n')[1].startswith(b'\xff\xfe'))
        self.assertIn('lines=2', case['source'])

    def test_generic_utf16_and_utf32_fragments_have_only_one_initial_bom(self):
        cases = audit.build_cases()
        self.assertEqual(len(cases), len({case['id'] for case in cases}))
        for name, bom in [('utf-16', b'\xfe\xff'), ('utf-32', b'\0\0\xfe\xff')]:
            case = next(case for case in cases if case['id'] == f'order-{name}-valid-all')
            data = bytes.fromhex(case['binary_resources']['part.adoc'])
            self.assertTrue(data.startswith(bom))
            self.assertEqual(data.count(bom), 1)

    def test_double_bom_probes_hold_two_boms_in_verbatim_content(self):
        cases = audit.double_bom_cases()
        self.assertEqual(len(cases), len({case['id'] for case in cases}))
        for prefix, bom in [('double-bom-utf-8-', b'\xef\xbb\xbf'), ('double-bom-utf-16le-', b'\xff\xfe'),
                            ('double-bom-utf-16be-', b'\xfe\xff'), ('double-bom-utf-32be-', b'\0\0\xfe\xff')]:
            case = next(case for case in cases if case['id'].startswith(prefix))
            self.assertTrue(bytes.fromhex(case['binary_resources']['part.adoc']).startswith(bom * 2))
            self.assertTrue(case['source'].startswith('----\n'))

    def test_uri_mode_serves_the_same_bytes_to_a_uri_include(self):
        case = audit.make_case('id', b'\xff\n', 'Windows-1251', 'lines=1')
        uri = audit.uri_case(case)
        self.assertEqual(uri['source'], 'include::{uri}/part.adoc[encoding=Windows-1251,lines=1,opts=optional]')
        self.assertEqual(uri['responses'], {'/part.adoc': [{'hex': 'ff0a'}]})
        self.assertEqual(uri['requests'], ['/part.adoc'])

    def test_cache_modes_enable_cache_uri_and_prime_a_hit_without_converting(self):
        case = audit.make_case('id', b'\xff\n', 'Windows-1251', 'lines=1')
        miss = audit.uri_case(case, 'miss')
        self.assertEqual(miss['source'], ':cache-uri:\n\n' + audit.uri_case(case)['source'])
        hit = audit.uri_case(case, 'hit')
        self.assertEqual(hit['source'], ':cache-uri:\n\ninclude::{uri}/part.adoc[lines=0]\n\n'
                         + audit.uri_case(case)['source'])
        # One request: the probed include of a hit must not reach the server.
        self.assertEqual(hit['requests'], ['/part.adoc'])
        self.assertEqual(hit['responses'], {'/part.adoc': [{'hex': 'ff0a'}]})

    def test_uri_mode_quarantines_ruby_errors_and_compares_requests(self):
        case = dict(requests=['/part.adoc'])
        request = [{'path': '/part.adoc', 'status': 200, 'sha256': 'abc'}]
        report = dict(html='<p>a</p>', diagnostics=[], requests=request)
        self.assertEqual(audit.compare_uri(case, report, report)['status'], 'PASS')
        error = dict(report, error={'class': 'ArgumentError', 'message': 'invalid'})
        self.assertEqual(audit.compare_uri(case, error, report)['status'], 'RUBY_ERROR')
        self.assertEqual(audit.compare_uri(case, error, dict(report, requests=[]))['status'], 'DIFF')
        self.assertEqual(audit.compare_uri(case, report, dict(report, html='<p>b</p>'))['status'], 'DIFF')
        self.assertEqual(audit.compare_uri(case, report, dict(report, diagnostics=[{}]))['status'], 'DIFF')


if __name__ == '__main__':
    unittest.main()
