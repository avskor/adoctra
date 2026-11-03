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


if __name__ == '__main__':
    unittest.main()
