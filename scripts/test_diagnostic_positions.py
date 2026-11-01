import contextlib
import copy
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import acceptance
import diagnostic_positions as positions


def report(file='<stdin>', line=3):
    return {'diagnostics': [{'severity': 'warning', 'message': 'warning text',
                             'location': {'file': file, 'line': line}}]}


class PositionTests(unittest.TestCase):
    def test_compat_cli_cursor_is_compared_without_fabricating_columns(self):
        stderr = 'adoctra: -:3: warning: warning text\n'
        self.assertEqual(positions.cli_projection(stderr), report())
        self.assertEqual(acceptance.cli_diagnostics(stderr),
                         [{'severity': 'warning', 'message': 'warning text'}])
        for changed in [stderr.replace(':3:', ':4:'), 'adoctra: warning: warning text\n']:
            self.assertNotEqual(positions.cli_projection(changed), report())
        with self.assertRaises(ValueError):
            positions.cli_projection(stderr.replace(':3:', ':0:'))

    def test_paths_preserve_nested_unicode_source_and_stdin(self):
        raw = report('/tmp/fixture/sub/я.adoc')
        self.assertEqual(positions.reference_projection(raw, Path('/tmp/fixture')),
                         report('<fixture>/sub/я.adoc'))
        self.assertEqual(positions.source_name('-'), '<stdin>')
        for path in ['/tmp/fixture-other/a.adoc', '/tmp/fixture/../a.adoc',
                     '/tmp/fixture/./a.adoc', '/tmp/fixture//a.adoc', 'a.adoc']:
            with self.subTest(path=path), self.assertRaises(ValueError):
                positions.source_name(path, Path('/tmp/fixture'))

    def test_cli_keeps_order_duplicates_and_unlocated_backend_warnings(self):
        line = 'adoctra: /tmp/fixture/sub/я.adoc:3:4: warning at 8..17: warning text\n'
        # All parser sources belong to the fixture; informational messages are
        # still parsed strictly before applying the normal WARN threshold.
        result = positions.cli_projection(line + line + 'adoctra: warning: backend\n'
                                          + 'adoctra: info: hidden\n', Path('/tmp/fixture'))
        diagnostic = report('<fixture>/sub/я.adoc')['diagnostics'][0]
        self.assertEqual(result['diagnostics'], [diagnostic, diagnostic,
                         {'severity': 'warning', 'message': 'backend', 'location': None}])

    def test_absence_of_cursor_is_not_inferred_from_message(self):
        raw = {'diagnostics': [{'severity': 'warning', 'message': 'at line 8 of sub/a.adoc', 'location': None}]}
        self.assertEqual(positions.reference_projection(copy.deepcopy(raw)), raw)
        self.assertNotEqual(raw, report())

    def test_schema_rejects_unknown_fields_bad_lines_and_paths(self):
        variants = [{}, {'diagnostics': {}}, {'diagnostics': [None]}]
        for value in [0, -1, True, 1.5, '3']:
            variants.append(report(line=value))
        for name in ['', '/tmp/a.adoc', '<fixture>/../a', '<fixture>//a', '<fixture>/a\\b']:
            variants.append(report(file=name))
        for field in ['file', 'line']:
            value = report()
            del value['diagnostics'][0]['location'][field]
            variants.append(value)
        value = report()
        value['diagnostics'][0]['location']['column'] = 1
        variants.append(value)
        value = report()
        del value['diagnostics'][0]['location']
        variants.append(value)
        for value in variants:
            with self.subTest(value=value), self.assertRaises(ValueError):
                positions.validate(value)

    def test_malformed_cli_output_cannot_become_empty_diagnostics(self):
        for stderr in ['noise', 'adoctra: -:0:1: warning at 0..1: x',
                       'adoctra: -:1:0: warning at 0..1: x',
                       'adoctra: -:1:1: warning at 2..1: x',
                       'adoctra: warning: first\ncontinuation']:
            with self.subTest(stderr=stderr), self.assertRaises(ValueError):
                positions.cli_projection(stderr)


class PositionHarnessTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / 'reference').mkdir()
        self.reference = self.root / 'reference/case.json'
        self.reference.write_text(json.dumps(report()))
        self.case = {'id': 'case', 'category': 'diagnostics', 'source': 'input', 'expect': 'pass'}

    def run_case(self, stderr='adoctra: -:3:2: warning at 8..12: warning text\n', **options):
        output = subprocess.CompletedProcess([], 0, '<p>irrelevant HTML</p>', stderr)
        with patch('acceptance.invoke', return_value=output) as invoke, contextlib.redirect_stdout(io.StringIO()):
            result = acceptance.check_case(self.case, Path('adoctra'), self.root,
                                           profile=positions.PROFILE, **options)
        return result, invoke

    def test_offline_and_coordinate_only_regressions(self):
        result, invoke = self.run_case()
        self.assertEqual(result['status'], 'PASS')
        self.assertEqual(invoke.call_count, 1)
        self.assertNotEqual(invoke.call_args.args[0][0], 'ruby')
        for stderr in ['adoctra: -:4:2: warning at 8..12: warning text\n',
                       'adoctra: warning: warning text\n']:
            self.assertEqual(self.run_case(stderr)[0]['status'], 'FAIL')
        self.reference.write_text(json.dumps(report('<fixture>/sub/a.adoc')))
        self.assertEqual(self.run_case()[0]['status'], 'FAIL')

    def test_xfail_freezes_positions_and_detects_xpass(self):
        self.case.update(expect='xfail', reason='known line difference')
        stderr = 'adoctra: -:4:1: warning at 8..12: warning text\n'
        self.assertEqual(self.run_case(stderr, record=True)[0]['status'], 'XFAIL')
        self.assertEqual(self.run_case(stderr)[0]['status'], 'XFAIL')
        self.assertEqual(self.run_case(stderr.replace(':4:', ':5:'))[0]['status'], 'FAIL')
        self.assertEqual(self.run_case()[0]['status'], 'XPASS')
        (self.root / 'baseline/case.json').write_text(json.dumps(report(line=0)))
        with self.assertRaises(ValueError):
            self.run_case(stderr)

    def test_update_uses_position_mode_and_verify_rejects_location_drift(self):
        output = subprocess.CompletedProcess([], 0, json.dumps(report()), '')
        with patch('acceptance.invoke', return_value=output) as invoke:
            acceptance.check_case(self.case, Path('adoctra'), self.root,
                                  profile=positions.PROFILE, update=True)
            self.assertIn('--diagnostic-positions', invoke.call_args.args[0])
            self.assertEqual(invoke.call_count, 1)
        output.stdout = json.dumps(report(line=4))
        with patch('acceptance.invoke', return_value=output), self.assertRaisesRegex(ValueError, 'reference drift'):
            acceptance.check_case(self.case, Path('adoctra'), self.root,
                                  profile=positions.PROFILE, verify=True)

    def test_corpus_declares_all_pinned_inputs(self):
        manifest, cases = acceptance.load_corpus(acceptance.CORPUS / 'acceptance/positions.json')
        self.assertEqual(manifest['profile'], positions.PROFILE)
        self.assertEqual(len(cases), 19)
        xfails = [c['id'] for c in cases if c['expect'] != 'pass']
        self.assertEqual(xfails, ['position-comment-direct-peek'])
        self.assertTrue(all(c.get('reason') for c in cases if c['expect'] == 'xfail'))
        self.assertTrue(any('\r\n' in c['source'] for c in cases))


if __name__ == '__main__':
    unittest.main()
