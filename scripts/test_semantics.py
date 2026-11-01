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


def table():
    cell = {'style': None, 'halign': 'left', 'valign': 'top', 'colspan': None, 'rowspan': None,
            'background': None, 'paragraphs': [[{'name': 'text', 'value': 'First.'}], [{'name': 'text', 'value': 'Second.'}]]}
    return {'name': 'table', 'attributes': {}, 'options': [], 'caption': None, 'width': 100, 'frame': 'all',
            'grid': 'all', 'stripes': None,
            'columns': [{'style': None, 'halign': 'left', 'valign': 'top', 'width': '100', 'autowidth': False}],
            'head': [], 'body': [[cell]], 'foot': []}


class SemanticProjectionTests(unittest.TestCase):
    def test_footnote_markers_catalog_order_and_content_are_independent(self):
        definition = {'name': 'footnote', 'type': 'definition', 'index': '1', 'id': 'n', 'parts': [text('Note')]}
        source = document()
        source['blocks'][0]['inlines'] += [definition, dict(definition, type='reference', id=None, parts=[])]
        source['footnotes'] = [{'index': '1', 'id': 'n', 'parts': [text('Note')]},
                               {'index': '1', 'id': 'unused', 'parts': [text('Stored')]}]
        original = semantics.document_projection(source, [])
        self.assertEqual(len(original['footnotes']), 2)  # Duplicate counter values are legal.
        for change in ['order', 'content', 'id', 'index', 'missing', 'marker']:
            changed = copy.deepcopy(source)
            if change == 'order': changed['footnotes'].reverse()
            elif change == 'content': changed['footnotes'][0]['parts'] = [text('Different')]
            elif change in {'id', 'index'}: changed['footnotes'][0][change] = 'other'
            elif change == 'missing': del changed['footnotes']
            else: changed['blocks'][0]['inlines'][1]['index'] = '2'
            self.assertNotEqual(original, semantics.document_projection(changed, []), change)
        self.assertEqual(semantics.parts_projection([dict(kind='footnote', **{k: v for k, v in definition.items() if k != 'name'})]),
                         semantics.inline_projection([definition]))

    def test_nested_footnote_catalog_does_not_leak_to_root(self):
        source = document()
        nested = table()
        cell = nested['body'][0][0]
        del cell['paragraphs']
        cell['blocks'] = []
        cell['style'] = 'asciidoc'
        cell['footnotes'] = [{'index': '2', 'id': 'n', 'parts': [text('Nested')]}]
        source['blocks'] = [nested]
        original = semantics.document_projection(source, [])
        self.assertNotIn('footnotes', original)
        self.assertEqual(original['blocks'][0]['body'][0][0]['document']['footnotes'][0]['index'], '2')
        changed = copy.deepcopy(source)
        changed['footnotes'] = changed['blocks'][0]['body'][0][0].pop('footnotes')
        self.assertNotEqual(original, semantics.document_projection(changed, []))

    def test_callout_ranges_address_utf8_text_and_preserve_literal_markers(self):
        mark = {'number': '1', 'id': 'CO1-1', 'guard': '//', 'xml': False, 'icons': None, 'src': '',
                'text_range': {'start': 16, 'end': 22}}
        source = {'name': 'listing', 'text': 'Ж🙂 <1>\nreal // <1>', 'callouts': [mark]}
        original = semantics.block_projection(source)
        semantics.validate_block(original)
        self.assertEqual(original['inlines'][0], text('Ж🙂 <1>\nreal '))
        self.assertEqual(original['inlines'][1]['kind'], 'callout')
        for field, value in [('number', '2'), ('id', 'CO1-2'), ('guard', '#'), ('icons', 'font'), ('src', '1.png'),
                             ('text_range', {'start': 7, 'end': 10})]:
            changed = copy.deepcopy(source)
            changed['callouts'][0][field] = value
            self.assertNotEqual(original, semantics.block_projection(changed), field)
        xml = copy.deepcopy(source)
        xml['callouts'][0].update(xml=True, guard=None)
        self.assertNotEqual(original, semantics.block_projection(xml))

    def test_callout_ranges_reject_overlap_invalid_types_and_split_codepoints(self):
        mark = {'number': '1', 'id': 'CO1-1', 'guard': None, 'xml': False, 'icons': None, 'src': '',
                'text_range': {'start': 3, 'end': 6}}
        source = {'name': 'listing', 'text': 'Ж <1>', 'callouts': [mark]}
        for interval in [{'start': 1, 'end': 6}, {'start': -1, 'end': 6}, {'start': 3, 'end': 9},
                         {'start': 3, 'end': 3}, {'start': True, 'end': 6}, {'start': 3, 'end': 6, 'extra': 1}]:
            changed = copy.deepcopy(source)
            changed['callouts'][0]['text_range'] = interval
            with self.subTest(interval=interval), self.assertRaises(ValueError):
                semantics.block_projection(changed)
        with self.assertRaises(ValueError):
            semantics.block_projection(dict(source, callouts=[mark, mark]))
        with self.assertRaises(ValueError):
            semantics.block_projection(dict(source, inlines=[]))

    def test_callout_substitution_parts_and_list_bindings_keep_semantics(self):
        mark = {'kind': 'callout', 'number': '1', 'id': 'CO1-1', 'guard': None, 'xml': False, 'icons': '', 'src': 'icons/1.png'}
        self.assertEqual(semantics.parts_projection([mark]), [mark])
        source = {'name': 'colist', 'style': 'arabic', 'items': [
            {'inlines': [{'name': 'text', 'value': 'One'}], 'blocks': [], 'coids': ['CO1-1', 'CO1-2'], 'icons': '', 'src': 'icons/1.png'}]}
        original = semantics.block_projection(source)
        semantics.validate_block(original)
        for field, value in [('coids', ['CO1-2', 'CO1-1']), ('coids', []), ('icons', 'font'), ('src', 'other.png')]:
            changed = copy.deepcopy(source)
            changed['items'][0][field] = value
            self.assertNotEqual(original, semantics.block_projection(changed))

    def test_footnote_and_callout_schema_rejects_inconsistent_or_unknown_fields(self):
        note = {'kind': 'footnote', 'type': 'reference', 'index': '1', 'id': None, 'inlines': []}
        mark = {'kind': 'callout', 'number': '1', 'id': 'CO1-1', 'guard': None, 'xml': False, 'icons': None, 'src': ''}
        for invalid in [dict(note, extra=1), dict(note, index=1), dict(note, type='other'), dict(note, id='n'),
                        dict(note, inlines=[text('bad')]), dict(note, type='unresolved'), dict(mark, number=1),
                        dict(mark, xml=1), dict(mark, xml=True, guard='//'), dict(mark, icons=True), dict(mark, extra=1)]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                semantics.validate_inlines([invalid])
        for entries in [[], [{}], [{'index': '1', 'id': None, 'inlines': [], 'extra': 1}]]:
            with self.assertRaises(ValueError):
                semantics.validate_footnotes({'footnotes': entries})

    def test_reference_footnote_catalog_decodes_entities_once(self):
        source = document()
        source['footnotes'] = [{'index': '1', 'id': 'A&amp;B', 'parts': [text('&lt;')]}]
        expected = semantics.document_projection(source, [])
        raw = copy.deepcopy(expected)
        raw['footnotes'][0].update(id='A&amp;B', inlines=[text('&amp;lt;')])
        self.assertEqual(expected, semantics.reference_projection(raw, []))

    def test_generated_xref_labels_and_selected_style_remain_semantic(self):
        parts = [{'kind': 'open-xref', 'target': '#s', 'refid': 's', 'xrefstyle': 'full'},
                 text('Section 1, “'), {'kind': 'open', 'format': 'strong'}, text('Title'),
                 {'kind': 'close', 'format': 'strong'}, text('”'), {'kind': 'close-xref'}]
        projected = semantics.parts_projection(parts)
        self.assertEqual(projected[0]['inlines'][1]['kind'], 'strong')
        semantics.validate_inlines(projected)
        changed = copy.deepcopy(parts)
        changed[1]['value'] = 'Section 2, “'
        self.assertNotEqual(projected, semantics.parts_projection(changed))
        with self.assertRaisesRegex(ValueError, 'xref metadata'):
            semantics.parts_projection([dict(parts[0], role='custom'), *parts[1:]])

    def test_indexterms_preserve_visibility_hierarchy_and_cross_references(self):
        node = {'name': 'indexterm', 'parts': None, 'terms': ['A &amp; B', 'Leaf'], 'see': 'Other', 'see_also': ['First', 'Second']}
        original = semantics.inline_projection([node])
        semantics.validate_inlines(original)
        self.assertEqual(original[0]['terms'], ['A & B', 'Leaf'])
        self.assertIsNone(original[0]['inlines'])
        for field, value in [('terms', ['Leaf', 'A &amp; B']), ('see', None), ('see_also', ['Second', 'First'])]:
            self.assertNotEqual(original, semantics.inline_projection([dict(node, **{field: value})]), field)
        visible = dict(node, parts=[{'kind': 'open', 'format': 'strong'}, text('Visible'), {'kind': 'close', 'format': 'strong'}], terms=[])
        projected = semantics.inline_projection([visible])
        semantics.validate_inlines(projected)
        self.assertEqual(projected[0]['inlines'][0]['kind'], 'strong')
        self.assertNotEqual(semantics.inline_projection([dict(visible, parts=[])]), semantics.inline_projection([dict(visible, parts=None)]))
        self.assertEqual(semantics.parts_projection([dict(kind='indexterm', **{key: value for key, value in visible.items() if key != 'name'})]), projected)

    def test_stem_inline_and_block_whitespace_is_never_folded(self):
        node = {'name': 'stem', 'style': 'asciimath', 'parts': [text('a  +\n  b')]}
        original = semantics.canonical(semantics.inline_projection([node]))
        semantics.validate_inlines(original)
        self.assertEqual(original[0]['inlines'], [text('a  +\n  b')])
        self.assertNotEqual(original, semantics.inline_projection([dict(node, style='latexmath')]))
        self.assertNotEqual(original, semantics.inline_projection([dict(node, parts=[text('a + b')])]))
        block = {'name': 'stem', 'style': 'latexmath', 'inlines': [{'name': 'text', 'value': 'a  < b\n  + c'}]}
        projected = semantics.block_projection(block)
        semantics.validate_block(projected)
        self.assertEqual(projected['inlines'], [text('a  < b\n  + c')])

    def test_new_semantic_nodes_reject_unknown_fields_and_invalid_types(self):
        term = {'kind': 'indexterm', 'inlines': None, 'terms': ['A'], 'see': None, 'see_also': []}
        stem = {'kind': 'stem', 'style': 'latexmath', 'inlines': [text('x')]}
        for invalid in [dict(term, extra=True), dict(term, terms=[None]), dict(term, see=[]),
                        dict(term, see_also='x'), dict(term, inlines=[]),
                        dict(stem, style='unknown'), dict(stem, inlines=None), dict(stem, extra=True)]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                semantics.validate_inlines([invalid])
        for invalid in [{'kind': 'stem', 'style': 'asciimath', 'parts': [], 'extra': True},
                        {'kind': 'indexterm', 'parts': None, 'terms': ['<b>x</b>'], 'see': None, 'see_also': []}]:
            with self.assertRaises(ValueError):
                semantics.parts_projection([invalid])

    def test_formula_and_indexterm_reference_decoding_is_single_pass(self):
        doc = document()
        doc['blocks'][0]['inlines'] = [
            {'name': 'stem', 'style': 'asciimath', 'parts': [text('a  &amp;lt;\n  b')]},
            {'name': 'indexterm', 'parts': None, 'terms': ['A &amp; B'], 'see': 'C &amp; D', 'see_also': ['&amp;lt;']}]
        oracle = semantics.document_projection(doc, ['a'])
        # Native metadata is lexical; the oracle independently starts with its
        # own lexical values, prior to the reference adapter's decoding pass.
        oracle['blocks'][0]['inlines'][1].update(terms=['A &amp; B'], see='C &amp; D', see_also=['&amp;lt;'])
        projected = semantics.reference_projection(oracle, ['a'])['blocks'][0]['inlines']
        self.assertEqual(projected[0]['inlines'], [text('a  &lt;\n  b')])
        self.assertEqual(projected[1]['terms'], ['A & B'])
        self.assertEqual(projected[1]['see'], 'C & D')
        self.assertEqual(projected[1]['see_also'], ['&lt;'])
        self.assertEqual(oracle['blocks'][0]['inlines'][0]['inlines'], [text('a  &amp;lt;\n  b')])

    def test_tables_keep_columns_groups_spans_alignment_and_paragraph_boundaries(self):
        node = table()
        original = semantics.block_projection(node)
        semantics.validate_block(original)
        cell = original['body'][0][0]
        self.assertEqual(cell['paragraphs'], [[text('First.')], [text('Second.')]])
        self.assertEqual((cell['colspan'], cell['rowspan']), (1, 1))
        for field, value in [('style', 'strong'), ('halign', 'right'), ('valign', 'middle'),
                             ('colspan', 2), ('rowspan', 2), ('background', 'red'),
                             ('paragraphs', [[{'name': 'text', 'value': 'First.Second.'}]])]:
            changed = copy.deepcopy(node)
            changed['body'][0][0][field] = value
            self.assertNotEqual(original, semantics.block_projection(changed), field)
        for field, value in [('style', 'emphasis'), ('halign', 'center'), ('valign', 'bottom'),
                             ('width', '50'), ('autowidth', True)]:
            changed = copy.deepcopy(node)
            changed['columns'][0][field] = value
            self.assertNotEqual(original, semantics.block_projection(changed), field)
        changed = copy.deepcopy(node)
        changed['foot'], changed['body'] = changed['body'], []
        self.assertNotEqual(original, semantics.block_projection(changed))

    def test_table_presentation_fields_are_compared(self):
        node = table()
        original = semantics.block_projection(node)
        self.assertEqual({key: original[key] for key in semantics.TABLE_FIELDS},
                         {'caption': None, 'frame': 'all', 'grid': 'all', 'stripes': None, 'width': 100,
                          'explicit_width': False, 'autowidth': False, 'float': None})
        for field, value in [('caption', 'Table 1. '), ('frame', 'ends'), ('grid', 'rows'), ('stripes', 'even'),
                             ('width', 50), ('attributes', {'width': '100%'}), ('attributes', {'float': 'left'}),
                             ('options', ['autowidth'])]:
            changed = dict(copy.deepcopy(node), **{field: value})
            projected = semantics.block_projection(changed)
            semantics.validate_block(projected)
            self.assertNotEqual(original, projected, field)
        # An explicit caption keeps its parts; an empty caption differs from none.
        parts = semantics.block_projection(dict(node, caption='X ', caption_parts=[{'kind': 'raw', 'value': 'X &amp; '}]))
        self.assertEqual(parts['caption'], [text('X & ')])
        self.assertEqual(semantics.block_projection(dict(node, caption=''))['caption'], [])

    def test_header_cells_are_single_inline_runs_including_empty_content(self):
        node = table()
        node['head'] = [[dict(node['body'][0][0], paragraphs=[])]]
        projected = semantics.block_projection(node)
        self.assertEqual(projected['head'][0][0]['inlines'], [])
        node['head'][0][0]['paragraphs'] = [[{'name': 'strong', 'inlines': [{'name': 'text', 'value': 'H'}]}]]
        self.assertEqual(semantics.block_projection(node)['head'][0][0]['inlines'][0]['kind'], 'strong')
        node['head'][0][0]['paragraphs'] *= 2
        with self.assertRaisesRegex(ValueError, 'header cell'):
            semantics.block_projection(node)

    def test_literal_cells_preserve_whitespace_and_csv_substitutions(self):
        cell = dict(table()['body'][0][0], style='literal')
        del cell['paragraphs']
        cell['literal'] = '  < &\n    next'
        result = semantics.cell_projection(cell, 'body')
        self.assertEqual(result['literal'], [text('  < &\n    next')])
        del cell['literal']
        cell['paragraphs'] = [[{'name': 'strong', 'inlines': [{'name': 'text', 'value': 'Ж'}]},
                               {'name': 'text', 'value': ' < 2\n    next'}]]
        result = semantics.cell_projection(cell, 'body')
        self.assertEqual(result['literal'][0]['kind'], 'strong')
        self.assertEqual(result['literal'][1], text(' < 2\n    next'))
        changed = copy.deepcopy(cell)
        changed['paragraphs'][0][1]['value'] = ' < 2     next'
        self.assertNotEqual(result, semantics.cell_projection(changed, 'body'))

    def test_asciidoc_cells_keep_nested_documents_tables_and_reject_invalid_footnotes(self):
        cell = dict(table()['body'][0][0], style='asciidoc')
        del cell['paragraphs']
        cell['blocks'] = [table()]
        result = semantics.cell_projection(cell, 'body')
        self.assertIsNone(result['document']['title'])
        self.assertEqual(result['document']['blocks'][0]['body'][0][0]['paragraphs'][0], [text('First.')])
        for invalid in [dict(cell, footnotes=[{}]), dict(cell, literal='extra'), dict(cell, future=True)]:
            with self.assertRaises(ValueError):
                semantics.cell_projection(invalid, 'body')

    def test_table_schema_rejects_unknown_fields_styles_and_invalid_spans(self):
        valid = semantics.block_projection(table())
        for field, value in [('style', 'future'), ('halign', 'future'), ('valign', 'future'),
                             ('colspan', 0), ('rowspan', True), ('background', []),
                             ('paragraphs', [None]), ('future', True)]:
            invalid = copy.deepcopy(valid)
            invalid['body'][0][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                semantics.validate_block(invalid)
        for field, value in [('caption', 'Table 1.'), ('frame', None), ('grid', 1), ('stripes', True),
                             ('width', '100'), ('width', 101), ('explicit_width', None), ('autowidth', 1),
                             ('float', [])]:
            invalid = dict(copy.deepcopy(valid), **{field: value})
            with self.subTest(table=field), self.assertRaises(ValueError):
                semantics.validate_block(invalid)
        for field, value in [('width', 50), ('width', 'nan'), ('autowidth', 1), ('future', True)]:
            invalid = copy.deepcopy(valid)
            invalid['columns'][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                semantics.validate_block(invalid)

    def test_reference_table_text_is_decoded_once_at_every_depth(self):
        doc = document()
        node = table()
        normal = node['body'][0][0]
        normal['paragraphs'] = [[{'name': 'text', 'value': '&amp;lt;'}]]
        node['head'] = [[copy.deepcopy(normal)]]
        nested = dict(normal, style='asciidoc', blocks=[copy.deepcopy(node)])
        del nested['paragraphs']
        literal = dict(normal, style='literal', literal='  &lt;\n    &amp;')
        del literal['paragraphs']
        node['body'].append([literal, nested])
        doc['blocks'] = [node]
        oracle = semantics.document_projection(doc, ['a'])
        result = semantics.reference_projection(oracle, ['a'])['blocks'][0]
        self.assertEqual(result['head'][0][0]['inlines'], [text('&lt;')])
        self.assertEqual(result['body'][0][0]['paragraphs'], [[text('&lt;')]])
        self.assertEqual(result['body'][1][0]['literal'], [text('  <\n    &')])
        self.assertEqual(result['body'][1][1]['document']['blocks'][0]['body'][0][0]['paragraphs'], [[text('&lt;')]])
        self.assertEqual(oracle['blocks'][0]['body'][0][0]['paragraphs'], [[text('&amp;lt;')]])

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

    def test_link_metadata_is_decoded_and_each_field_affects_comparison(self):
        fields = {'id': 'home', 'role': 'primary wide', 'title': 'A &amp; B', 'window': '_blank', 'rel': 'nofollow noopener'}
        parts = [dict(kind='open-link', target='https://example.org', **fields), text('Site'), {'kind': 'close-link'}]
        projected = semantics.parts_projection(parts)
        self.assertEqual(projected[0]['metadata'], dict(fields, title='A & B'))
        for field in fields:
            changed = copy.deepcopy(parts)
            changed[0][field] = 'different'
            self.assertNotEqual(projected, semantics.parts_projection(changed), field)
        parts[0]['future'] = 'value'
        with self.assertRaisesRegex(ValueError, 'link metadata'):
            semantics.parts_projection(parts)

    def test_ui_keys_menus_and_formatted_buttons_are_semantic_trees(self):
        button = {'kind': 'button', 'parts': [{'kind': 'open', 'format': 'strong'}, text('Save'), {'kind': 'close', 'format': 'strong'}]}
        keyboard = {'kind': 'kbd', 'keys': [[text('Ctrl')], [text('S')]]}
        menu = {'kind': 'menu', 'menu': [text('File')], 'submenus': [[text('New')]], 'item': [text('Project')], 'font': False}
        projected = semantics.parts_projection([button, keyboard, menu])
        self.assertEqual(projected[0]['inlines'][0]['kind'], 'strong')
        self.assertEqual(projected[1], {'kind': 'kbd', 'keys': [[text('Ctrl')], [text('S')]]})
        self.assertEqual(projected[2], {k: v for k, v in menu.items() if k != 'font'})
        native = [{'name': node['kind'], **{k: v for k, v in node.items() if k != 'kind'}} for node in [button, keyboard, menu]]
        self.assertEqual(semantics.inline_projection(native), projected)
        self.assertNotEqual(semantics.parts_projection([dict(keyboard, keys=list(reversed(keyboard['keys'])))]), [projected[1]])
        self.assertNotEqual(semantics.parts_projection([dict(menu, item=None)]), semantics.parts_projection([dict(menu, item=[])]))
        self.assertNotEqual(semantics.parts_projection([dict(menu, submenus=[])]), [projected[2]])
        self.assertNotEqual(semantics.parts_projection([dict(button, parts=[text('Save')])]), [projected[0]])

    def test_image_fields_alt_and_metadata_survive_projection_and_mutations(self):
        node = {'name': 'image', 'target': 'cat.png', 'src': 'assets/cat.png', 'alt': 'Cat &amp; friend',
                'attributes': {'id': 'cat', 'role': 'zoom wide', 'width': '24', 'link': 'https://example.org', 'align': 'center'},
                'parts': [{'kind': 'open', 'format': 'strong'}, text('Cat'), {'kind': 'close', 'format': 'strong'}, {'kind': 'raw', 'value': ' &amp; friend'}]}
        projected = semantics.inline_projection([node])
        self.assertEqual(projected[0]['id'], 'cat')
        self.assertEqual(projected[0]['roles'], ['zoom', 'wide'])
        self.assertEqual(projected[0]['alt'][0]['kind'], 'strong')
        self.assertEqual(projected[0]['alt'][1], text(' & friend'))
        for field in ['target', 'src']:
            self.assertNotEqual(projected, semantics.inline_projection([dict(node, **{field: 'different'})]))
        for field in node['attributes']:
            changed = copy.deepcopy(node)
            changed['attributes'][field] = 'different'
            self.assertNotEqual(projected, semantics.inline_projection([changed]), field)
        self.assertNotEqual(projected, semantics.inline_projection([dict(node, parts=[text('Cat & friend')])]))

    def test_icons_compare_name_mode_uri_alt_and_fields(self):
        node = {'name': 'image', 'icon': True, 'target': 'heart', 'src': 'ic/heart.png', 'alt': 'A & B', 'icons': '',
                'attributes': {'size': '2x', 'title': 'T', 'role': 'red', 'id': 'h'}, 'options': [],
                'parts': [{'kind': 'raw', 'value': 'A &amp; B'}]}
        projected = semantics.inline_projection([node])
        semantics.validate_inlines(projected)
        self.assertEqual(projected[0], {'kind': 'icon', 'id': 'h', 'roles': ['red'], 'target': 'heart', 'icons': '',
                                        'src': 'ic/heart.png', 'alt': [text('A & B')],
                                        'attributes': {'size': '2x', 'title': 'T'}})
        for field, value in [('target', 'x'), ('src', 'x.png'), ('icons', 'font'), ('icons', None),
                             ('attributes', {'size': '3x'}), ('parts', [text('C')])]:
            self.assertNotEqual(projected, semantics.inline_projection([dict(node, **{field: value})]), field)
        for invalid in [dict(node, attributes={'custom': 'x'}), dict(node, options=['inline']), dict(node, icons=1)]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                semantics.inline_projection([invalid])
        with self.assertRaises(ValueError):
            semantics.validate_inlines([dict(projected[0], attributes={'float': 'left'})])

    def test_complex_inline_unsupported_options_fields_and_raw_labels_fail(self):
        image = {'name': 'image', 'target': 'cat.png', 'src': 'cat.png', 'alt': 'Cat'}
        for invalid in [dict(image, icon=True), dict(image, options=['inline']), dict(image, svg={'mode': 'inline'}),
                        dict(image, attributes={'custom': 'x'}), dict(image, future=True),
                        {'name': 'button', 'parts': [], 'future': True},
                        {'name': 'kbd', 'keys': [[{'kind': 'raw', 'value': '<b>x</b>'}]]}]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                semantics.inline_projection([invalid])

    def test_lists_keep_checkboxes_descriptions_and_nested_blocks(self):
        item = {'inlines': [{'name': 'text', 'value': 'Done'}], 'checkbox': True, 'blocks': document('Attached.')['blocks']}
        listing = {'name': 'list', 'ordered': False, 'items': [item]}
        projected = semantics.block_projection(listing)
        self.assertEqual(projected['context'], 'ulist')
        self.assertEqual(projected['items'][0]['checkbox'], True)
        self.assertEqual(projected['items'][0]['blocks'][0]['inlines'], [text('Attached.')])
        for field, value in [('checkbox', False), ('blocks', []), ('inlines', [])]:
            self.assertNotEqual(projected, semantics.block_projection(dict(listing, items=[dict(item, **{field: value})])))
        description = {'name': 'dlist', 'items': [{'terms': [[{'name': 'text', 'value': 'First'}], [{'name': 'text', 'value': 'Alias'}]], 'description': item},
                                               {'terms': [[{'name': 'text', 'value': 'Last'}]], 'description': None}]}
        projected = semantics.block_projection(description)
        self.assertEqual(projected['items'][0]['terms'], [[text('First')], [text('Alias')]])
        self.assertIsNone(projected['items'][1]['description'])
        changed = copy.deepcopy(description)
        changed['items'][0]['terms'].reverse()
        self.assertNotEqual(projected, semantics.block_projection(changed))

    def test_simple_and_compound_containers_remain_distinct(self):
        simple = {'name': 'admonition', 'admonition-type': 'note', 'simple': True, 'blocks': document()['blocks']}
        projected = semantics.block_projection(simple)
        self.assertEqual(projected['inlines'], [text('Text.')])
        self.assertEqual(projected['type'], 'note')
        self.assertNotIn('blocks', projected)
        compound = semantics.block_projection(dict(simple, simple=False))
        self.assertEqual(compound['blocks'][0]['inlines'], projected['inlines'])
        self.assertNotIn('inlines', compound)
        for invalid in [dict(simple, blocks=[]), dict(simple, blocks=[dict(simple['blocks'][0], id='hidden')])]:
            with self.assertRaises(ValueError):
                semantics.block_projection(invalid)

    def test_quote_and_verse_keep_credit_content_model_and_verse_whitespace(self):
        credit = {'author': [{'name': 'raw', 'value': 'Ann &amp; Bob'}], 'citetitle': [{'name': 'text', 'value': 'Book'}]}
        quote = {'name': 'quote', 'attribution': credit, 'blocks': document('One.')['blocks'] * 2}
        projected = semantics.block_projection(quote)
        self.assertEqual(projected['attribution'], [text('Ann & Bob')])
        self.assertEqual(projected['citetitle'], [text('Book')])
        self.assertEqual(len(projected['blocks']), 2)
        semantics.validate_block(projected)
        for changed in [dict(credit, citetitle=None), dict(credit, author=[{'name': 'text', 'value': 'Eve'}])]:
            self.assertNotEqual(projected, semantics.block_projection(dict(quote, attribution=changed)))
        bare = semantics.block_projection(dict(quote, attribution=None))
        self.assertIsNone(bare['attribution'])
        self.assertIsNone(bare['citetitle'])
        simple = semantics.block_projection(dict(quote, simple=True, blocks=document('Only.')['blocks']))
        self.assertEqual(simple['inlines'], [text('Only.')])
        verse = {'name': 'verse', 'attribution': None, 'inlines': [{'name': 'text', 'value': '  one\n    two'}]}
        projected = semantics.block_projection(verse)
        self.assertEqual(projected['inlines'], [text('  one\n    two')])
        semantics.validate_block(projected)
        self.assertNotEqual(projected, semantics.block_projection(dict(verse, inlines=[{'name': 'text', 'value': 'one two'}])))
        with self.assertRaisesRegex(ValueError, 'attribution'):
            semantics.block_projection(dict(quote, attribution={'author': None, 'future': None}))

    def test_toc_lists_the_sections_with_the_captions_adoctra_resolved(self):
        def section(level, ident, title, children=()):
            return {'name': 'section', 'level': level, 'blocks': [
                {'name': 'heading', 'level': level, 'id': ident, 'caption': None, 'inlines': title}, *children]}
        sub = section(2, '_sub', [{'name': 'text', 'value': 'Sub'}])
        link = {'name': 'xref', 'target': '#x', 'refid': 'x', 'inlines': [{'name': 'text', 'value': 'link'}]}
        note = {'name': 'footnote', 'type': 'definition', 'index': '1', 'id': 'n', 'parts': [text('N')]}
        top = section(1, '_s', [{'name': 'text', 'value': 'S '}, link, {'name': 'anchor', 'id': 'a'}, note], [sub])
        toc = {'name': 'toc', 'id': 't', 'roles': ['r'], 'title': [{'name': 'text', 'value': 'Contents'}],
               'toc': {'levels': 2, 'entries': [{'caption': '1. ', 'sections': [{'caption': None, 'sections': []}]}]}}
        doc = document()
        doc['blocks'] = [toc, top]
        doc['attribute_snapshots']['final']['values'].update({'toc': '', 'toc-title': 'Ignored'})
        projected = semantics.document_projection(doc, [])
        self.assertIsNone(projected['outline'])
        outline = projected['blocks'][0]['outline']
        self.assertEqual(outline['title'], [text('Contents')])  # The macro's title wins.
        self.assertEqual(outline['levels'], 2)
        entry = outline['sections'][0]
        self.assertEqual(entry['id'], '_s')
        # Links and anchors lose their element; a footnote keeps its number.
        self.assertEqual(entry['inlines'], [text('1. S link'), {'kind': 'superscript', 'id': '_footnote_n',
                                                                 'roles': ['footnote'], 'inlines': [text('[1]')]}])
        self.assertEqual(entry['sections'], [{'id': '_sub', 'inlines': [text('Sub')], 'sections': []}])
        # A disabled macro, and entries that do not follow the sections.
        disabled = copy.deepcopy(doc)
        disabled['blocks'][0]['toc'] = None
        self.assertIsNone(semantics.document_projection(disabled, [])['blocks'][0]['outline'])
        broken = copy.deepcopy(doc)
        broken['blocks'][0]['toc']['entries'].append({'caption': None, 'sections': []})
        with self.assertRaisesRegex(ValueError, 'TOC entries'):
            semantics.document_projection(broken, [])
        # The automatic TOC takes toc-title, including its parts.
        auto = copy.deepcopy(doc)
        auto['blocks'] = [top]
        auto['toc'] = {'levels': 1, 'entries': [{'caption': 'X ', 'caption_parts': [{'kind': 'raw', 'value': 'X &amp; '}],
                                                 'sections': []}]}
        auto['attribute_snapshots']['final']['attribute_parts'] = {'toc-title': [{'kind': 'raw', 'value': 'A &amp; B'}]}
        outline = semantics.document_projection(auto, [])['outline']
        self.assertEqual(outline['title'], [text('A & B')])
        self.assertEqual(outline['sections'][0]['inlines'][0], text('X & S link'))
        with self.assertRaisesRegex(ValueError, 'toc'):
            semantics.block_projection(dict(toc, blocks=document()['blocks']))
        projected = semantics.block_projection(dict(toc, toc=None))
        with self.assertRaises(ValueError):
            semantics.validate_block(dict(projected, outline=None, inlines=[]))
        for invalid in [{'title': [], 'levels': '2', 'sections': []}, {'title': [], 'levels': 2},
                        {'title': [], 'levels': 2, 'sections': [{'id': None, 'inlines': []}]}]:
            with self.assertRaises(ValueError):
                semantics.validate_outline(invalid)

    def test_reference_quote_credit_and_verse_are_decoded_once(self):
        oracle = {'attributes': {'header': {}, 'final': {}}, 'title': None, 'outline': None, 'blocks': [
            {'context': 'quote', 'id': None, 'roles': [], 'title': None, 'attribution': [text('A &amp;amp; B')],
             'citetitle': None, 'inlines': [text('Q')]},
            {'context': 'verse', 'id': None, 'roles': [], 'title': None, 'attribution': None, 'citetitle': [text('T')],
             'inlines': [text('  a &lt;\n b')]}]}
        projected = semantics.reference_projection(oracle, [])
        self.assertEqual(projected['blocks'][0]['attribution'], [text('A &amp; B')])
        self.assertEqual(projected['blocks'][1]['inlines'], [text('  a <\n b')])
        self.assertEqual(oracle['blocks'][0]['attribution'], [text('A &amp;amp; B')])

    def test_verbatim_whitespace_and_language_are_not_normalized_away(self):
        node = {'name': 'listing', 'text': '  x < 2\n    y', 'attributes': {'language': 'ruby'}}
        projected = semantics.block_projection(node)
        self.assertEqual(projected['inlines'], [text('  x < 2\n    y')])
        self.assertEqual(projected['language'], 'ruby')
        self.assertNotEqual(projected, semantics.block_projection(dict(node, text='x < 2 y')))
        self.assertNotEqual(projected, semantics.block_projection(dict(node, attributes={'language': 'python'})))
        with self.assertRaisesRegex(ValueError, 'callouts'):
            semantics.block_projection(dict(node, callouts=[1]))

    def test_expanded_oracle_schema_decodes_labels_and_preserves_code_spacing(self):
        doc = document()
        doc['blocks'] = [{'name': 'listing', 'text': '  x &lt; 2\n    y'},
                         {'name': 'image', 'target': 'cat.png', 'src': 'cat.png', 'alt': 'A &amp; B'}]
        oracle = semantics.document_projection(doc, ['a'])
        oracle['title'] = [{'kind': 'menu', 'menu': [text('A  &amp; B')], 'submenus': [], 'item': None}]
        result = semantics.reference_projection(oracle, ['a'])
        self.assertEqual(result['title'][0]['menu'], [text('A  & B')])
        self.assertEqual(result['blocks'][0]['inlines'], [text('  x < 2\n    y')])
        self.assertEqual(result['blocks'][1]['alt'], [text('A & B')])
        for invalid in [dict(oracle['title'][0], submenus=[None]), dict(oracle['title'][0], extra=1),
                        {'kind': 'image', 'id': None, 'roles': [], 'target': 'x', 'src': 'x', 'alt': [], 'attributes': {'width': 24}}]:
            with self.assertRaises(ValueError):
                semantics.validate_inlines([invalid])

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
            semantics.block_projection({'name': 'future'})

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
        self.assertEqual(len(cases), 142)
        self.assertEqual(manifest['profile'], semantics.PROFILE)
        reference = json.loads((root / 'reference/semantic-attributes-macros-in-quotes.json').read_text())
        strong = reference['semantics']['blocks'][0]['inlines'][0]
        self.assertEqual(strong['kind'], 'strong')
        self.assertEqual(strong['inlines'][0], text('World '))
        self.assertEqual(strong['inlines'][1]['target'], 'https://example.org')

    def test_real_pages_reuse_pinned_sources_and_host_settings(self):
        root = CORPUS / 'acceptance'
        manifest, cases = acceptance.load_corpus(root / 'semantics.json')
        original_manifest, original = acceptance.load_corpus(root / 'cases.json')
        self.assertEqual(manifest['profile'], semantics.PROFILE)
        self.assertEqual(len(cases), 43)
        for path, provenance in manifest['files'].items():
            self.assertEqual(provenance, original_manifest['files'][path])
        for origin in manifest['origins'].values():
            if 'configuration' in origin:
                self.assertIn(origin['configuration'], manifest['files'])
        for case in cases:
            original_case = next(entry for entry in original if 'semantic-' + entry['id'] == case['id'])
            for key in ['source', 'attributes', 'resources']:
                self.assertEqual(case.get(key), original_case.get(key))
            self.assertGreater(len(case['source'].splitlines()), 90)
            self.assertEqual(case['expect'], 'pass')
            reference = json.loads((root / 'reference' / (case['id'] + '.json')).read_text())
            semantics.validate(reference['semantics'], case['semantic_attributes'])


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
