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
from compatibility import CORPUS, pretty
import semantics


def text(value):
    return {'kind': 'text', 'value': value}


def document(value='Text.'):
    return {'name': 'document', 'blocks': [{'name': 'paragraph', 'inlines': [{'name': 'text', 'value': value}]}],
            'attribute_snapshots': {'header': {'values': {'a': 'before'}}, 'final': {'values': {'a': 'after'}}}}


class SemanticProjectionTests(unittest.TestCase):
    def test_text_merging_preserves_order_and_whitespace_policy(self):
        self.assertEqual(semantics.canonical([text('a '), text('\n b')]), [text('a b')])
        self.assertEqual(semantics.canonical([text('a '), text('\n b')], prose=False), [text('a \n b')])
        self.assertEqual(semantics.canonical([text(''), {'kind': 'line-break'}, text('')]), [{'kind': 'line-break'}])

    def test_lexical_entities_are_decoded_once_without_html_parsing(self):
        self.assertEqual(semantics.lexical('&amp;lt; &copy; &#169;'), '&lt; © ©')
        self.assertEqual(semantics.inline_projection([{'name': 'text', 'value': '&amp;'}]), [text('&amp;')])
        self.assertEqual(semantics.inline_projection([{'name': 'raw', 'value': '&amp;'}]), [text('&')])
        with self.assertRaisesRegex(ValueError, 'raw markup'):
            semantics.lexical('<em>x</em>')

    def test_parts_can_cross_inline_nodes_and_retain_format_metadata(self):
        nodes = [{'name': 'substituted', 'parts': [{'kind': 'open', 'format': 'strong', 'id': 'i', 'role': 'r s'}]},
                 {'name': 'emphasis', 'inlines': [{'name': 'text', 'value': 'x'}]},
                 {'name': 'substituted', 'parts': [{'kind': 'close', 'format': 'strong'}]}]
        result = semantics.inline_projection(nodes)
        self.assertEqual(result[0]['id'], 'i')
        self.assertEqual(result[0]['roles'], ['r', 's'])
        self.assertEqual(result[0]['inlines'][0]['kind'], 'emphasis')
        for broken in [nodes[:-1], nodes[1:], [dict(nodes[0], parts=[{'kind': 'close', 'format': 'strong'}])]]:
            with self.assertRaisesRegex(ValueError, 'unbalanced'):
                semantics.inline_projection(broken)

    def test_mutations_to_text_type_target_role_and_nesting_remain_visible(self):
        original = [{'name': 'link', 'target': 'https://example.org', 'inlines': [{'name': 'strong', 'inlines': [{'name': 'text', 'value': 'x'}]}]}]
        variants = []
        for field, value in [('target', 'different'), ('name', 'xref')]:
            changed = copy.deepcopy(original)
            changed[0][field] = value
            variants.append(changed)
        changed = copy.deepcopy(original)
        changed[0]['inlines'][0]['inlines'][0]['value'] = 'y'
        variants.append(changed)
        changed = copy.deepcopy(original)
        changed[0]['inlines'][0]['name'] = 'emphasis'
        variants.append(changed)
        for variant in variants:
            self.assertNotEqual(semantics.inline_projection(original), semantics.inline_projection(variant))
        self.assertNotEqual(semantics.canonical([text('a'), {'kind': 'line-break'}, text('b')]), [text('a b')])

    def test_snapshots_keep_absent_empty_and_final_values_distinct(self):
        doc = document()
        doc['attribute_snapshots']['header']['values']['empty'] = ''
        result = semantics.document_projection(doc, ['a', 'empty', 'absent'])
        self.assertEqual(result['attributes']['header'], {'a': [text('before')], 'empty': [], 'absent': None})
        self.assertEqual(result['attributes']['final']['a'], [text('after')])
        with self.assertRaisesRegex(ValueError, 'selection mismatch'):
            semantics.validate(result, ['different'])

    def test_formatted_attribute_snapshots_are_trees_not_flat_values(self):
        doc = document()
        doc['attribute_snapshots']['header']['attribute_parts'] = {'a': [
            {'kind': 'open', 'format': 'strong'}, {'kind': 'text', 'value': 'before'}, {'kind': 'close', 'format': 'strong'}]}
        projected = semantics.document_projection(doc, ['a'])
        self.assertEqual(projected['attributes']['header']['a'][0]['kind'], 'strong')

    def test_unknown_nodes_raw_markup_and_malformed_schema_fail_closed(self):
        for node in [{'name': 'future'}, {'name': 'raw', 'value': '<b>x</b>'}, {'name': 'image'},
                     {'name': 'substituted', 'parts': [{'kind': 'future'}]},
                     {'name': 'substituted', 'parts': [{'kind': 'text', 'value': 'x', 'future': True}]}]:
            with self.assertRaises(ValueError):
                semantics.inline_projection([node])
        valid = semantics.document_projection(document(), ['a'])
        for invalid in [dict(valid, extra=1), dict(valid, blocks=[{'context': 'future'}]), dict(valid, title=[{'kind': 'future'}])]:
            with self.assertRaises(ValueError):
                semantics.validate(invalid, ['a'])
        with self.assertRaises(ValueError):
            semantics.block_projection({'name': 'table'})

    def test_oracle_normalization_does_not_mutate_or_decode_twice(self):
        original = semantics.document_projection(document('&amp;lt;'), ['a'])
        changed = semantics.reference_projection(original, ['a'])
        self.assertEqual(changed['blocks'][0]['inlines'], [text('&lt;')])
        self.assertEqual(original['blocks'][0]['inlines'], [text('&amp;lt;')])

    def test_manifest_selection_and_reserved_characters_are_validated(self):
        for names in [None, ['a', 'a'], ['Bad'], ['a!']]:
            with self.assertRaises(ValueError):
                semantics.validate_case({'source': 'x', 'semantic_attributes': names})
        for field in ['source', 'resources', 'attributes']:
            case = {'source': 'x', 'semantic_attributes': []}
            case[field] = '\ue0000\ue001' if field == 'source' else {'x': '\ue0000\ue001'}
            with self.assertRaisesRegex(ValueError, 'reserved'):
                semantics.validate_case(case)

    def test_pinned_fixture_expands_attributes_and_macros_inside_quotes(self):
        root = CORPUS / 'upstream'
        manifest, cases = acceptance.load_corpus(root / 'semantics.json')
        self.assertEqual(len(cases), 38)
        self.assertEqual(manifest['profile'], semantics.PROFILE)
        reference = json.loads((root / 'reference/semantic-attributes-macros-in-quotes.json').read_text())
        strong = reference['semantics']['blocks'][0]['inlines'][0]
        self.assertEqual(strong['kind'], 'strong')
        self.assertEqual(strong['inlines'][0], text('World '))
        self.assertEqual(strong['inlines'][1]['target'], 'https://example.org')


class SemanticExecutionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / 'reference').mkdir()
        self.case = {'id': 'semantic', 'category': 'semantics', 'source': 'Text.', 'expect': 'pass', 'semantic_attributes': ['a']}
        self.actual = document()
        self.reference = {'semantics': semantics.document_projection(self.actual, ['a'])}
        self.path = self.root / 'reference/semantic.json'
        self.path.write_text(pretty(self.reference))

    def run_case(self, actual=None, stderr='', **options):
        output = subprocess.CompletedProcess([], 0, json.dumps(self.actual if actual is None else actual), stderr)
        with patch('acceptance.invoke', return_value=output) as invoke, contextlib.redirect_stdout(io.StringIO()):
            result = acceptance.check_case(self.case, Path('adoctra'), self.root, profile=semantics.PROFILE, **options)
        return result, invoke

    def test_offline_uses_json_attribute_snapshots_without_ruby(self):
        result, invoke = self.run_case()
        self.assertEqual(result['status'], 'PASS')
        self.assertEqual(result['checks'], {'semantics': 'PASS'})
        self.assertEqual(invoke.call_count, 1)
        command = invoke.call_args.args[0]
        self.assertIn('--json-attributes', command)
        self.assertEqual(command[command.index('--to') + 1], 'json')

    def test_update_drift_and_attribute_selection_use_semantic_oracle(self):
        _, invoke = self.run_case(self.reference, update=True)
        command = invoke.call_args.args[0]
        self.assertIn('--semantics', command)
        self.assertEqual(command[command.index('--semantic-attribute') + 1], 'a')
        changed = copy.deepcopy(self.reference)
        changed['semantics']['attributes']['header']['a'] = [text('changed')]
        with self.assertRaisesRegex(ValueError, 'reference drift'):
            self.run_case(changed, verify=True)
        with self.assertRaisesRegex(ValueError, 'selection mismatch'):
            self.run_case({'semantics': semantics.document_projection(self.actual, [])}, update=True)

    def test_xfail_baseline_cannot_hide_regression_or_unexpected_fix(self):
        self.case.update(expect='xfail', reason='known semantic difference')
        changed = document('changed')
        self.assertEqual(self.run_case(changed, record=True)[0]['status'], 'XFAIL')
        self.assertEqual(self.run_case(changed)[0]['status'], 'XFAIL')
        self.assertEqual(self.run_case(document('other'))[0]['status'], 'FAIL')
        self.assertEqual(self.run_case()[0]['status'], 'XPASS')
        (self.root / 'baseline/semantic.json').write_text('{"semantics": {}}')
        with self.assertRaises(ValueError):
            self.run_case(changed)

    def test_warnings_and_wrong_reference_types_are_not_silently_dropped(self):
        with self.assertRaisesRegex(ValueError, 'diagnostics are outside'):
            self.run_case(stderr='adoctra: warning: test\n')
        self.path.write_text('{"structure": {}}')
        with self.assertRaisesRegex(ValueError, 'invalid semantic report'):
            self.run_case()

    def test_include_uses_the_same_isolated_tree(self):
        self.case.update(profile='include-body-dom-v1', resources={'part.adoc': 'Text.'})
        outputs = [subprocess.CompletedProcess([], 0, json.dumps(value), '') for value in [self.reference, self.actual]]
        with patch('acceptance.invoke', side_effect=outputs) as invoke, contextlib.redirect_stdout(io.StringIO()):
            acceptance.check_case(self.case, Path('adoctra'), self.root, profile=semantics.PROFILE, verify=True)
        ruby, rust = [call.args[0] for call in invoke.call_args_list]
        self.assertEqual(Path(rust[1]).parent, Path(ruby[ruby.index('--include-root') + 1]))
        self.assertIn('safe', rust)


if __name__ == '__main__':
    unittest.main()
