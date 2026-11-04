"""inline-semantics-v1: bounded semantic trees, independent of HTML rendering.

No HTML parser is used. Ruby emits nodes through a custom converter; adoctra
exports its resolved JSON AST. Unsupported nodes/raw markup fail closed.
"""
import copy
import html
import re

PROFILE = 'inline-semantics-v1'
PENDING_OUTLINE = '\ue000outline'
FORMATS = {'strong', 'emphasis', 'monospace', 'mark', 'superscript', 'subscript', 'span'}
CONTAINERS = {'section', 'preamble', 'open', 'sidebar', 'example', 'admonition', 'quote'}
QUOTES = {'quote', 'verse'}
LISTS = {'ulist', 'olist', 'dlist', 'colist'}
VERBATIM = {'listing', 'literal'}
IMAGE_ATTRIBUTES = {'width', 'height', 'title', 'link', 'float', 'align'}
ICON_ATTRIBUTES = {'size', 'flip', 'rotate', 'title', 'width', 'height', 'link', 'window'}
LINK_METADATA = {'id', 'role', 'title', 'window', 'rel'}
CELL_STYLES = {None, 'asciidoc', 'none', 'emphasis', 'header', 'literal', 'monospaced', 'strong'}


def require(condition, message='invalid inline-semantics-v1 projection'):
    if not condition:
        raise ValueError(message)


def lexical(value):
    require(isinstance(value, str))
    require('<' not in value, 'raw markup is outside inline-semantics-v1')
    return html.unescape(value)


def canonical(nodes, prose=True):
    result = []
    for node in nodes:
        node = copy.deepcopy(node)
        if node.get('inlines') is not None:
            node['inlines'] = canonical(node['inlines'], False if node['kind'] == 'stem' else prose)
        for key in ['alt', 'menu', 'item']:
            if node.get(key) is not None:
                node[key] = canonical(node[key], prose=False)
        for key in ['keys', 'submenus']:
            if key in node:
                node[key] = [canonical(label, prose=False) for label in node[key]]
        if node['kind'] == 'text':
            if not node['value']:
                continue
            if result and result[-1]['kind'] == 'text':
                result[-1]['value'] += node['value']
                continue
        result.append(node)
    if prose:
        for node in result:
            if node['kind'] == 'text':
                node['value'] = re.sub(r'[ \t\r\n]+', ' ', node['value'])
    return result


def parts_projection(parts):
    root = []
    stack = [(None, root)]
    append_parts(parts, stack)
    require(len(stack) == 1, 'unbalanced semantic parts')
    return root


def append_parts(parts, stack):
    for part in parts:
        kind = part['kind']
        if kind in {'text', 'raw'}:
            require(set(part) == {'kind', 'value'})
            stack[-1][1].append({'kind': 'text', 'value': lexical(part['value']) if kind == 'raw' else part['value']})
        elif kind in {'open', 'open-link', 'open-xref'}:
            if kind == 'open':
                require(set(part) <= {'kind', 'format', 'id', 'role'})
                require(part['format'] in FORMATS)
                node = {'kind': part['format'], 'id': lexical(part['id']) if 'id' in part else None,
                        'roles': lexical(part.get('role', '')).split(), 'inlines': []}
                closing = ('close', part['format'])
            else:
                allowed = {'kind', 'target', *LINK_METADATA}
                if kind == 'open-xref':
                    allowed = {'kind', 'target', 'refid', 'path', 'fragment', 'role', 'xrefstyle'}
                require(set(part) <= allowed, 'link metadata is outside inline-semantics-v1')
                node = {'kind': 'xref' if kind == 'open-xref' else 'link', 'target': lexical(part['target']), 'inlines': []}
                # An xref has only a role (the html5 converter writes no
                # window or rel for it).
                names = {'role'} if kind == 'open-xref' else LINK_METADATA
                fields = {key: lexical(part[key]) for key in names if key in part and not (key == 'role' and part[key] == 'bare')}
                if fields:
                    node['metadata'] = fields
                closing = ('close-xref' if kind == 'open-xref' else 'close-link', None)
            stack[-1][1].append(node)
            stack.append((closing, node['inlines']))
        elif kind in {'close', 'close-link', 'close-xref'}:
            require(set(part) == ({'kind', 'format'} if kind == 'close' else {'kind'}))
            require(len(stack) > 1 and stack[-1][0] == (kind, part.get('format')), 'unbalanced semantic parts')
            stack.pop()
        elif kind == 'line-break':
            require(set(part) == {'kind'})
            stack[-1][1].append({'kind': 'line-break'})
        elif kind == 'anchor':
            require(set(part) == {'kind', 'id'})
            stack[-1][1].append({'kind': 'anchor', 'id': lexical(part['id'])})
        elif kind in {'kbd', 'button', 'menu', 'image', 'indexterm', 'stem', 'footnote', 'callout'}:
            stack[-1][1].append(complex_inline(part, kind))
        else:
            raise ValueError(f'unsupported semantic part: {kind}')


def inline_projection(nodes):
    result = []
    stack = [(None, result)]
    for node in nodes:
        name = node['name']
        if name in {'text', 'substituted'}:
            append_parts(node['parts'] if node.get('parts') else [{'kind': 'text', 'value': node['value']}], stack)
        elif name == 'raw':
            stack[-1][1].append({'kind': 'text', 'value': lexical(node['value'])})
        elif name in FORMATS:
            stack[-1][1].append({'kind': name, 'id': None, 'roles': [], 'inlines': inline_projection(node['inlines'])})
        elif name in {'link', 'xref'}:
            require(name == 'xref' or not node.get('role'), 'link role is outside inline-semantics-v1')
            projected = {'kind': name, 'target': node['target'], 'inlines': inline_projection(node['inlines'])}
            if node.get('role'):
                projected['metadata'] = {'role': lexical(node['role'])}
            stack[-1][1].append(projected)
        elif name == 'anchor':
            stack[-1][1].append({'kind': name, 'id': node['id']})
        elif name in {'kbd', 'button', 'menu', 'image', 'indexterm', 'stem', 'footnote', 'callout'}:
            stack[-1][1].append(complex_inline(node, name))
        else:
            raise ValueError(f'unsupported semantic inline: {name}')
    require(len(stack) == 1, 'unbalanced semantic parts')
    return result


def svg_projection(svg):
    """The SVG embedding: the read file (raw markup, compared exactly) or the
    interactive fallback's source."""
    if svg is None:
        return None
    require(isinstance(svg, dict), 'invalid svg')
    if svg.get('mode') == 'inline':
        require(set(svg) == {'mode', 'content'} and (svg['content'] is None or isinstance(svg['content'], str)), 'invalid svg')
        return dict(svg)
    require(svg.get('mode') == 'interactive' and set(svg) == {'mode', 'fallback'}, 'invalid svg')
    return {'mode': 'interactive', 'fallback': lexical(svg['fallback']) if svg['fallback'] is not None else None}


def image_fields(node):
    require(all(isinstance(node.get(key), str) for key in ['target', 'src', 'alt']), 'invalid image fields')
    require(set(node.get('options', [])) <= {'inline', 'interactive'} and not node.get('icon'),
            'image options/icons are outside inline-semantics-v1')
    attrs = node.get('attributes', {})
    # pdfwidth/scaledwidth size the image in PDF and DocBook only (as in the
    # Ruby reference converter, they are accepted and not compared).
    require(set(attrs) <= IMAGE_ATTRIBUTES | {'id', 'role', 'alt', 'fallback', 'format', 'pdfwidth', 'scaledwidth'},
            'image attributes are outside inline-semantics-v1')
    return {'target': lexical(node['target']), 'src': lexical(node['src']),
            'alt': parts_projection(node['parts']) if 'parts' in node else [{'kind': 'text', 'value': lexical(node['alt'])}],
            'attributes': {key: lexical(value) for key, value in attrs.items() if key in IMAGE_ATTRIBUTES},
            'svg': svg_projection(node.get('svg'))}


def complex_inline(node, kind):
    fields = {'image': {'icon', 'target', 'src', 'alt', 'icons', 'attributes', 'options', 'parts', 'svg'},
              'button': {'parts'}, 'kbd': {'keys'}, 'menu': {'menu', 'submenus', 'item', 'font'},
              'indexterm': {'parts', 'terms', 'see', 'see_also'}, 'stem': {'style', 'parts'},
              'footnote': {'type', 'index', 'id', 'parts'},
              'callout': {'number', 'id', 'guard', 'xml', 'icons', 'src'}}[kind]
    require(set(node) <= fields | {'name', 'kind'}, 'unsupported complex inline field')
    if kind == 'footnote':
        return {'kind': kind, 'type': node['type'], 'index': node['index'],
                'id': lexical(node['id']) if node['id'] is not None else None,
                'inlines': parts_projection(node['parts'])}
    if kind == 'callout':
        return {'kind': kind, **{key: node[key] for key in fields}}
    if kind == 'stem':
        return {'kind': kind, 'style': node['style'], 'inlines': parts_projection(node['parts'])}
    if kind == 'indexterm':
        return {'kind': kind, 'inlines': parts_projection(node['parts']) if node['parts'] is not None else None,
                'terms': [lexical(term) for term in node['terms']],
                'see': lexical(node['see']) if node['see'] is not None else None,
                'see_also': [lexical(term) for term in node['see_also']]}
    if kind == 'image' and node.get('icon'):
        attrs = node.get('attributes', {})
        require(set(attrs) <= ICON_ATTRIBUTES | {'id', 'role', 'alt'} and not node.get('options') and not node.get('svg'),
                'icon options are outside inline-semantics-v1')
        require(isinstance(node.get('target'), str) and isinstance(node.get('src'), str) and 'icons' in node
                and (node['icons'] is None or isinstance(node['icons'], str)), 'invalid icon fields')
        return {'kind': 'icon', 'id': lexical(attrs['id']) if 'id' in attrs else None,
                'roles': lexical(attrs.get('role', '')).split(), 'target': lexical(node['target']),
                'icons': node['icons'], 'src': lexical(node['src']),
                'alt': parts_projection(node['parts']) if 'parts' in node else [{'kind': 'text', 'value': lexical(node['alt'])}],
                'attributes': {key: lexical(value) for key, value in attrs.items() if key in ICON_ATTRIBUTES}}
    if kind == 'image':
        fields = image_fields(node)
        attrs = node.get('attributes', {})
        return {'kind': kind, 'id': lexical(attrs['id']) if 'id' in attrs else None,
                'roles': lexical(attrs.get('role', '')).split(), **fields}
    if kind == 'button':
        return {'kind': kind, 'inlines': parts_projection(node['parts'])}
    if kind == 'kbd':
        return {'kind': kind, 'keys': [parts_projection(label) for label in node['keys']]}
    return {'kind': kind, 'menu': parts_projection(node['menu']),
            'submenus': [parts_projection(label) for label in node['submenus']],
            'item': parts_projection(node['item']) if node['item'] is not None else None,
            'font': node.get('font') is True}


def item_projection(item, callout=False):
    result = {'inlines': canonical(inline_projection(item['inlines'])), 'checkbox': item.get('checkbox'),
              'blocks': [block_projection(child) for child in item['blocks']]}
    if callout:
        result.update({key: item[key] for key in ['coids', 'icons', 'src']})
    return result


def footnotes_projection(entries):
    result = []
    for entry in entries:
        require(isinstance(entry, dict) and set(entry) == {'index', 'id', 'parts'}, 'invalid footnote definition')
        result.append({'index': entry['index'], 'id': lexical(entry['id']) if entry['id'] is not None else None,
                       'inlines': canonical(parts_projection(entry['parts']))})
    return result


def verbatim_projection(block):
    if 'inlines' in block:
        require(not block.get('callouts'), 'ambiguous verbatim content')
        return inline_projection(block['inlines'])
    source = block['text'].encode('utf-8')
    result, previous = [], 0
    require(isinstance(block.get('callouts', []), list), 'invalid callouts')
    for callout in block.get('callouts', []):
        require(isinstance(callout, dict) and 'text_range' in callout, 'invalid callouts entry')
        interval = callout['text_range']
        require(isinstance(interval, dict) and set(interval) == {'start', 'end'}, 'invalid callout text range')
        start, end = interval['start'], interval['end']
        require(type(start) is int and type(end) is int and previous <= start < end <= len(source), 'invalid callout text range')
        # Offsets address the converted UTF-8 block text, not Python characters
        # or original source spans. Decode each boundary to reject split codepoints.
        source[start:end].decode('utf-8')
        result.append({'kind': 'text', 'value': source[previous:start].decode('utf-8')})
        result.append(complex_inline({key: value for key, value in callout.items() if key != 'text_range'}, 'callout'))
        previous = end
    result.append({'kind': 'text', 'value': source[previous:].decode('utf-8')})
    return result


def cell_projection(cell, group):
    keys = {'style', 'halign', 'valign', 'colspan', 'rowspan', 'background'}
    content = set(cell) & {'paragraphs', 'blocks', 'literal'}
    require(len(content) == 1 and set(cell) <= keys | content | {'footnotes', 'toc'}, 'invalid semantic cell')
    require('toc' not in cell or 'blocks' in cell, 'a TOC requires a nested document')
    require('footnotes' not in cell or 'blocks' in cell, 'footnotes require a nested document')
    result = {key: cell[key] for key in keys}
    for key in ['colspan', 'rowspan']:
        result[key] = 1 if result[key] is None else result[key]
    if group == 'head':
        require('paragraphs' in cell and len(cell['paragraphs']) <= 1, 'invalid header cell')
        result['inlines'] = canonical(inline_projection(cell['paragraphs'][0]) if cell['paragraphs'] else [])
    elif 'blocks' in cell:
        nested = document_projection({'name': 'document', 'blocks': cell['blocks'], 'footnotes': cell.get('footnotes', []),
                                      'attribute_snapshots': {'header': {'values': {}}, 'final': {'values': {}}}}, [])
        result['document'] = {key: nested[key] for key in ['title', 'blocks']}
        if 'footnotes' in nested:
            result['document']['footnotes'] = nested['footnotes']
        if cell.get('toc') is not None:
            # A nested automatic TOC: depth and listed sections (its title
            # and class are compared by the DOM profile).
            toc = cell['toc']
            require(isinstance(toc, dict) and set(toc) == {'levels', 'entries'}, 'invalid nested TOC')
            listed = cell['blocks'][1:] if nested['title'] is not None else cell['blocks']
            result['document']['outline'] = {'levels': toc['levels'],
                                             'sections': outline_projection(listed, toc['entries'])}
    elif 'literal' in cell:
        result['literal'] = canonical([{'kind': 'text', 'value': cell['literal']}], prose=False)
    elif cell['style'] == 'literal':
        # CSV/DSV literal cells retain normal substitutions in the JSON AST.
        require(len(cell['paragraphs']) <= 1, 'invalid literal cell')
        result['literal'] = canonical(inline_projection(cell['paragraphs'][0]) if cell['paragraphs'] else [], prose=False)
    else:
        result['paragraphs'] = [canonical(inline_projection(paragraph)) for paragraph in cell['paragraphs']]
    return result


def block_projection(block):
    context = block['name']
    if context == 'list':
        context = 'olist' if block['ordered'] else 'ulist'
    if context == 'heading':
        # A discrete heading outside a section's own heading.
        require(block.get('style') in {'discrete', 'float'}, 'unexpected heading')
        return {'context': 'floating_title', 'id': block.get('id'), 'roles': block.get('roles', []),
                'level': block['level'], 'title': canonical(inline_projection(block['inlines']))}
    if context == 'break':
        require(block.get('variant') in {'thematic', 'page'} and not block.get('blocks'), 'invalid break')
        return {'context': f"{block['variant']}_break", 'id': block.get('id'), 'roles': block.get('roles', []),
                'title': canonical(inline_projection(block['title'])) if block.get('title') is not None else None}
    require(context in CONTAINERS | LISTS | VERBATIM | {'paragraph', 'image', 'table', 'stem', 'verse', 'toc'},
            f'unsupported semantic block: {context}')
    metadata = block
    children = block.get('blocks', [])
    if context == 'section':
        require(bool(children) and children[0]['name'] == 'heading' and children[0]['level'] == block['level'])
        metadata, children = children[0], children[1:]
        title = canonical(inline_projection(metadata['inlines']))
    else:
        title = canonical(inline_projection(block['title'])) if block.get('title') is not None else None
    result = {'context': context, 'id': metadata.get('id'), 'roles': metadata.get('roles', []), 'title': title}
    if context == 'section':
        result['level'] = block['level']
    if context == 'admonition':
        result['type'] = block['admonition-type']
    if context in QUOTES:
        credit = block.get('attribution') or {}
        require(set(credit) <= {'author', 'citetitle'}, 'invalid attribution')
        for key, field in [('author', 'attribution'), ('citetitle', 'citetitle')]:
            result[field] = canonical(inline_projection(credit[key])) if credit.get(key) is not None else None
    if context == 'toc':
        require(not children, 'unexpected toc content')
        # Filled by document_projection, which owns the listed sections.
        result['outline'] = {PENDING_OUTLINE: block['toc']}
    elif context == 'verse':
        result['inlines'] = canonical(inline_projection(block['inlines']), prose=False)
    elif context in LISTS:
        if context == 'dlist':
            result['items'] = [{'terms': [canonical(inline_projection(term)) for term in item['terms']],
                                'description': item_projection(item['description']) if item['description'] is not None else None}
                               for item in block['items']]
        else:
            result['style'] = block.get('style') or ('arabic' if context == 'olist' else None)
            result['items'] = [item_projection(item, callout=context == 'colist') for item in block['items']]
    elif context == 'table':
        if block.get('caption_parts') is not None:
            result['caption'] = canonical(parts_projection(block['caption_parts']), prose=False)
        else:
            result['caption'] = canonical([{'kind': 'text', 'value': lexical(block['caption'])}], prose=False) if block.get('caption') is not None else None
        attrs = block.get('attributes', {})
        result.update({key: block[key] for key in ['frame', 'grid', 'stripes', 'width']})
        result['explicit_width'] = 'width' in attrs
        result['autowidth'] = 'autowidth' in block.get('options', [])
        result['options'] = sorted(set(block.get('options', [])))
        result['float'] = lexical(attrs['float']) if 'float' in attrs else None
        result['columns'] = copy.deepcopy(block['columns'])
        for group in ['head', 'body', 'foot']:
            result[group] = [[cell_projection(cell, group) for cell in row] for row in block[group]]
    elif context == 'stem':
        result['style'] = block['style']
        result['inlines'] = canonical(inline_projection(block['inlines']), prose=False)
    elif context in VERBATIM:
        result['language'] = block.get('attributes', {}).get('language')
        result['inlines'] = canonical(verbatim_projection(block), prose=False)
    elif context == 'image':
        fields = image_fields(block)
        fields['alt'] = canonical(fields['alt'], prose=False)
        result.update(fields)
    elif context == 'paragraph':
        result['inlines'] = canonical(inline_projection(block['inlines']))
    elif block.get('simple', False):
        require(len(children) == 1 and children[0]['name'] == 'paragraph', 'invalid simple container')
        require(not any(children[0].get(key) for key in ['id', 'roles', 'title', 'attributes', 'options']), 'unexpected simple content metadata')
        result['inlines'] = canonical(inline_projection(children[0]['inlines']))
    else:
        result['blocks'] = [block_projection(child) for child in children]
    if context == 'preamble':
        result['outline'] = {PENDING_OUTLINE: block.get('toc')}
    return result


def strip_anchors(nodes):
    """Ruby's DropAnchorRx on a converted title, applied to semantic nodes.

    Links, xrefs and anchors lose their element and keep their content; a
    footnote marker keeps its visible superscript, as in Ruby's generated
    xref labels (see xref_label in asciidoctor-semantics.rb)."""
    result = []
    for node in nodes:
        if node['kind'] == 'anchor':
            continue
        if node['kind'] in {'link', 'xref'}:
            result.extend(strip_anchors(node['inlines']))
        elif node['kind'] == 'footnote' and node['index'] is not None:
            result.append({'kind': 'superscript', 'id': f"_footnote_{node['id']}" if node['id'] is not None else None,
                           'roles': ['footnote' if node['type'] == 'definition' else 'footnoteref'],
                           'inlines': [{'kind': 'text', 'value': f"[{node['index']}]"}]})
        elif node.get('inlines') is not None:
            result.append(dict(node, inlines=strip_anchors(node['inlines'])))
        else:
            result.append(node)
    return result


def outline_projection(blocks, entries):
    """The sections a TOC lists: adoctra resolved each caption when the TOC
    converted; the title is the section heading's converted title."""
    sections = [block for block in blocks if block['name'] == 'section']
    require(isinstance(entries, list) and len(entries) == len(sections), 'TOC entries do not follow the sections')
    result = []
    for block, entry in zip(sections, entries):
        require(isinstance(entry, dict) and set(entry) <= {'caption', 'caption_parts', 'sections'}, 'invalid TOC entry')
        heading = block['blocks'][0]
        require(heading['name'] == 'heading' and heading['level'] == block['level'], 'invalid section heading')
        if entry.get('caption_parts'):
            label = parts_projection(entry['caption_parts'])
        else:
            label = [{'kind': 'text', 'value': lexical(entry['caption'])}] if entry['caption'] else []
        label += inline_projection(heading['inlines'])
        children = entry['sections']
        result.append({'id': heading.get('id'), 'inlines': canonical(strip_anchors(canonical(label))),
                       'sections': outline_projection(block['blocks'][1:], children) if children else []})
    return result


def fill_outlines(node, document, values, parts):
    """Replace the pending TOC and preamble outlines with the TOCs adoctra
    converted; a TOC takes the macro's title, else `toc-title`."""
    if isinstance(node, list):
        for child in node:
            fill_outlines(child, document, values, parts)
        return
    if not isinstance(node, dict):
        return
    pending = node.get('outline')
    if isinstance(pending, dict) and PENDING_OUTLINE in pending:
        toc = pending[PENDING_OUTLINE]
        title = node['title'] if node['context'] == 'toc' else None
        role = ' '.join(node['roles']) if node['context'] == 'toc' and node['roles'] else None
        node['outline'] = document_outline(document, toc, values, parts, title, role) if toc is not None else None
    for key, value in node.items():
        if key != 'outline':
            fill_outlines(value, document, values, parts)


def document_outline(document, toc, values, parts, title=None, role=None):
    require(isinstance(toc, dict) and set(toc) == {'levels', 'entries'} and type(toc['levels']) is int, 'invalid TOC')
    if title is None:
        if 'toc-title' in parts:
            title = canonical(parts_projection(parts['toc-title']))
        else:
            title = canonical([{'kind': 'text', 'value': values.get('toc-title', '')}])
    # The html5 converter's class: the macro's role, else `toc-class` or `toc`.
    toc_class = role or lexical(values.get('toc-class', 'toc'))
    return {'title': title, 'levels': toc['levels'], 'class': toc_class,
            'sections': outline_projection(document['blocks'], toc['entries'])}


def document_projection(document, names):
    require(isinstance(document, dict) and document.get('name') == 'document')
    blocks = document['blocks']
    require(isinstance(blocks, list))
    title = None
    if blocks and blocks[0]['name'] == 'heading' and blocks[0]['level'] == 0 and blocks[0].get('style') not in {'discrete', 'float'}:
        title, blocks = canonical(inline_projection(blocks[0]['inlines'])), blocks[1:]
    attributes = {}
    for stage in ['header', 'final']:
        snapshot = document['attribute_snapshots'][stage]
        attributes[stage] = {}
        for name in names:
            if name not in snapshot['values']:
                value = None
            elif name in snapshot.get('attribute_parts', {}):
                value = canonical(parts_projection(snapshot['attribute_parts'][name]), prose=False)
            else:
                value = canonical([{'kind': 'text', 'value': snapshot['values'][name]}], prose=False)
            attributes[stage][name] = value
    result = {'attributes': attributes, 'title': title, 'outline': None,
              'blocks': [block_projection(block) for block in blocks]}
    final = document['attribute_snapshots']['final']
    values, parts = final['values'], final.get('attribute_parts', {})
    listed = dict(document, blocks=blocks)
    fill_outlines(result['blocks'], listed, values, parts)
    if document.get('toc') is not None:
        result['outline'] = document_outline(listed, document['toc'], values, parts)
    if document.get('footnotes'):
        result['footnotes'] = footnotes_projection(document['footnotes'])
    validate(result, names)
    return result


def reference_projection(result, names):
    """Decode Ruby's lexical substitution strings, never parse rendered HTML."""
    result = copy.deepcopy(result)

    def decode_svg(node):
        # The read file is raw markup on both sides; the fallback is a URI.
        if node.get('svg') and node['svg']['mode'] == 'interactive' and node['svg']['fallback'] is not None:
            node['svg']['fallback'] = lexical(node['svg']['fallback'])

    def inlines(nodes, prose=True):
        for node in nodes:
            if node['kind'] == 'text':
                node['value'] = lexical(node['value'])
            for key in ['id', 'target', 'src']:
                if node.get(key) is not None:
                    node[key] = lexical(node[key])
            decode_svg(node)
            if 'roles' in node:
                node['roles'] = [lexical(role) for role in node['roles']]
            if node.get('inlines') is not None:
                node['inlines'] = inlines(node['inlines'], False if node['kind'] == 'stem' else prose)
            if node['kind'] == 'indexterm':
                node['terms'] = [lexical(term) for term in node['terms']]
                node['see_also'] = [lexical(term) for term in node['see_also']]
                if node['see'] is not None:
                    node['see'] = lexical(node['see'])
            for key in ['metadata', 'attributes']:
                if key in node:
                    node[key] = {name: lexical(value) for name, value in node[key].items()}
            for key in ['alt', 'menu', 'item']:
                if node.get(key) is not None:
                    node[key] = inlines(node[key], False)
            for key in ['keys', 'submenus']:
                if key in node:
                    node[key] = [inlines(label, False) for label in node[key]]
        return canonical(nodes, prose)

    def block(node):
        if node['id'] is not None:
            node['id'] = lexical(node['id'])
        node['roles'] = [lexical(role) for role in node['roles']]
        if node['title'] is not None:
            node['title'] = inlines(node['title'])
        if 'inlines' in node:
            node['inlines'] = inlines(node['inlines'], node['context'] not in VERBATIM | {'stem', 'verse'})
        for key in ['attribution', 'citetitle']:
            if node.get(key) is not None:
                node[key] = inlines(node[key])
        if node['context'] == 'image':
            for key in ['target', 'src']:
                node[key] = lexical(node[key])
            node['alt'] = inlines(node['alt'], False)
            node['attributes'] = {key: lexical(value) for key, value in node['attributes'].items()}
            decode_svg(node)
        if node['context'] == 'table':
            if node['caption'] is not None:
                node['caption'] = inlines(node['caption'], False)
            for key in ['frame', 'grid', 'stripes', 'float']:
                if node[key] is not None:
                    node[key] = lexical(node[key])
            for group in ['head', 'body', 'foot']:
                for row in node[group]:
                    for cell in row:
                        if 'inlines' in cell:
                            cell['inlines'] = inlines(cell['inlines'])
                        elif 'paragraphs' in cell:
                            cell['paragraphs'] = [inlines(paragraph) for paragraph in cell['paragraphs']]
                        elif 'literal' in cell:
                            cell['literal'] = inlines(cell['literal'], False)
                        else:
                            nested = cell['document']
                            if nested['title'] is not None:
                                nested['title'] = inlines(nested['title'])
                            for child in nested['blocks']:
                                block(child)
                            footnotes(nested)
                            if nested.get('outline') is not None:
                                entries = list(nested['outline']['sections'])
                                while entries:
                                    entry = entries.pop()
                                    if entry['id'] is not None:
                                        entry['id'] = lexical(entry['id'])
                                    entry['inlines'] = inlines(entry['inlines'])
                                    entries.extend(entry['sections'])
        for entry in node.get('items', []):
            if node['context'] == 'dlist':
                entry['terms'] = [inlines(term) for term in entry['terms']]
                if entry['description'] is not None:
                    item(entry['description'])
            else:
                item(entry)
        outline(node)
        for child in node.get('blocks', []):
            block(child)

    def outline(node):
        if node.get('outline') is None:
            return
        node['outline']['title'] = inlines(node['outline']['title'])
        node['outline']['class'] = lexical(node['outline']['class'])
        pending = list(node['outline']['sections'])
        while pending:
            entry = pending.pop()
            if entry['id'] is not None:
                entry['id'] = lexical(entry['id'])
            entry['inlines'] = inlines(entry['inlines'])
            pending.extend(entry['sections'])

    def item(node):
        node['inlines'] = inlines(node['inlines'])
        if 'coids' in node:
            node['coids'] = [lexical(value) for value in node['coids']]
            node['src'] = lexical(node['src'])
        for child in node['blocks']:
            block(child)

    def footnotes(document):
        for entry in document.get('footnotes', []):
            if entry['id'] is not None:
                entry['id'] = lexical(entry['id'])
            entry['inlines'] = inlines(entry['inlines'])

    # Validate the raw oracle schema before walking it, then canonical output.
    validate(result, names)
    for stage in result['attributes'].values():
        for name, value in stage.items():
            if value is not None:
                stage[name] = inlines(value, prose=False)
    if result['title'] is not None:
        result['title'] = inlines(result['title'])
    outline(result)
    for node in result['blocks']:
        block(node)
    footnotes(result)
    validate(result, names)
    return result


def validate_inlines(nodes):
    require(isinstance(nodes, list))
    for node in nodes:
        require(isinstance(node, dict))
        kind = node.get('kind')
        keys = {'kind'}
        if kind == 'text':
            keys.add('value')
            require(isinstance(node.get('value'), str))
        elif kind in FORMATS:
            keys.update({'id', 'roles', 'inlines'})
            metadata(node)
        elif kind in {'link', 'xref'}:
            keys.update({'target', 'inlines'})
            require(isinstance(node.get('target'), str))
            if 'metadata' in node:
                keys.add('metadata')
                validate_strings(node['metadata'], LINK_METADATA if kind == 'link' else {'role'})
        elif kind == 'anchor':
            keys.add('id')
            require(isinstance(node.get('id'), str))
        elif kind == 'button':
            keys.add('inlines')
        elif kind == 'kbd':
            keys.add('keys')
            validate_labels(node.get('keys'))
        elif kind == 'menu':
            keys.update({'menu', 'submenus', 'item', 'font'})
            require(type(node.get('font')) is bool)
            validate_inlines(node.get('menu'))
            validate_labels(node.get('submenus'))
            if node.get('item') is not None:
                validate_inlines(node['item'])
        elif kind == 'image':
            keys.update({'id', 'roles', 'target', 'src', 'alt', 'attributes', 'svg'})
            metadata(node)
            validate_image(node)
        elif kind == 'icon':
            keys.update({'id', 'roles', 'target', 'icons', 'src', 'alt', 'attributes'})
            metadata(node)
            require(isinstance(node.get('target'), str))
            validate_icon(node)
            validate_inlines(node.get('alt'))
            validate_strings(node.get('attributes'), ICON_ATTRIBUTES)
        elif kind == 'stem':
            keys.update({'style', 'inlines'})
            require(node.get('style') in {'asciimath', 'latexmath'})
        elif kind == 'indexterm':
            keys.update({'terms', 'see', 'see_also', 'inlines'})
            for key in ['terms', 'see_also']:
                require(isinstance(node.get(key), list) and all(isinstance(term, str) for term in node[key]))
            require(node.get('see') is None or isinstance(node['see'], str))
            require(node.get('inlines') is None or node['terms'] == [])
        elif kind == 'footnote':
            keys.update({'type', 'index', 'id', 'inlines'})
            require(node.get('type') in {'definition', 'reference', 'unresolved'})
            require(node.get('id') is None or isinstance(node['id'], str))
            if node['type'] == 'unresolved':
                require(node.get('index') is None and isinstance(node['id'], str))
            else:
                require(isinstance(node.get('index'), str) and bool(node['index']))
            if node['type'] != 'definition':
                require(node.get('inlines') == [])
            if node['type'] == 'reference':
                require(node['id'] is None)
        elif kind == 'callout':
            keys.update({'number', 'id', 'guard', 'xml', 'icons', 'src'})
            require(isinstance(node.get('number'), str) and re.fullmatch(r'[0-9]+', node['number']))
            require(node.get('id') is None or isinstance(node['id'], str))
            require(node.get('guard') is None or isinstance(node['guard'], str))
            require(type(node.get('xml')) is bool and (not node['xml'] or node['guard'] is None))
            validate_icon(node)
        else:
            require(kind == 'line-break')
        require(set(node) == keys)
        if 'inlines' in keys and not (kind == 'indexterm' and node['inlines'] is None):
            validate_inlines(node['inlines'])


def metadata(node):
    require(node.get('id') is None or isinstance(node['id'], str))
    require(isinstance(node.get('roles'), list) and all(isinstance(role, str) for role in node['roles']))


def validate_strings(value, allowed):
    require(isinstance(value, dict) and set(value) <= allowed and all(isinstance(v, str) for v in value.values()))


def validate_labels(labels):
    require(isinstance(labels, list))
    for label in labels:
        validate_inlines(label)


def validate_image(node):
    require(all(isinstance(node.get(key), str) for key in ['target', 'src']))
    validate_inlines(node.get('alt'))
    validate_strings(node.get('attributes'), IMAGE_ATTRIBUTES)
    svg = node.get('svg')
    require(svg is None or (isinstance(svg, dict) and (
        (set(svg) == {'mode', 'content'} and svg['mode'] == 'inline' and (svg['content'] is None or isinstance(svg['content'], str)))
        or (set(svg) == {'mode', 'fallback'} and svg['mode'] == 'interactive'
            and (svg['fallback'] is None or isinstance(svg['fallback'], str))))), 'invalid svg')


def validate_icon(node):
    require(node.get('icons') is None or isinstance(node['icons'], str))
    require(isinstance(node.get('src'), str))


def validate_footnotes(node):
    if 'footnotes' not in node:
        return
    require(isinstance(node['footnotes'], list) and bool(node['footnotes']))
    for entry in node['footnotes']:
        require(isinstance(entry, dict) and set(entry) == {'index', 'id', 'inlines'})
        require(isinstance(entry['index'], str) and bool(entry['index']))
        require(entry['id'] is None or isinstance(entry['id'], str))
        validate_inlines(entry['inlines'])


def validate_item(item, callout=False):
    keys = {'inlines', 'checkbox', 'blocks'} | ({'coids', 'icons', 'src'} if callout else set())
    require(isinstance(item, dict) and set(item) == keys)
    if callout:
        require(isinstance(item['coids'], list) and all(isinstance(value, str) for value in item['coids']))
        validate_icon(item)
    require(item['checkbox'] is None or type(item['checkbox']) is bool)
    validate_inlines(item['inlines'])
    require(isinstance(item['blocks'], list))
    for block in item['blocks']:
        validate_block(block)


def validate_alignment(node):
    require(node.get('style') in CELL_STYLES)
    require(node.get('halign') in {'left', 'center', 'right'})
    require(node.get('valign') in {'top', 'middle', 'bottom'})


TABLE_FIELDS = {'caption', 'frame', 'grid', 'stripes', 'width', 'explicit_width', 'autowidth', 'float', 'options'}


def validate_table(node):
    if node.get('caption') is not None:
        validate_inlines(node['caption'])
    for key in ['frame', 'grid']:
        require(isinstance(node.get(key), str))
    for key in ['stripes', 'float']:
        require(node.get(key) is None or isinstance(node[key], str))
    require(type(node.get('width')) is int and 0 <= node['width'] <= 100)
    require(type(node.get('explicit_width')) is bool and type(node.get('autowidth')) is bool)
    require(isinstance(node.get('options'), list) and all(isinstance(o, str) for o in node['options']))
    require(isinstance(node.get('columns'), list) and bool(node['columns']))
    for column in node['columns']:
        require(isinstance(column, dict) and set(column) == {'style', 'halign', 'valign', 'width', 'autowidth'})
        validate_alignment(column)
        require(isinstance(column['width'], str) and re.fullmatch(r'[0-9]+(?:\.[0-9]+)?', column['width']))
        require(type(column['autowidth']) is bool)
    for group in ['head', 'body', 'foot']:
        require(isinstance(node.get(group), list))
        for row in node[group]:
            require(isinstance(row, list))
            for cell in row:
                require(isinstance(cell, dict))
                validate_alignment(cell)
                keys = {'style', 'halign', 'valign', 'colspan', 'rowspan', 'background'}
                for span in ['colspan', 'rowspan']:
                    require(type(cell.get(span)) is int and cell[span] > 0)
                require(cell.get('background') is None or isinstance(cell['background'], str))
                if group == 'head':
                    keys.add('inlines')
                    validate_inlines(cell.get('inlines'))
                elif cell['style'] == 'asciidoc':
                    keys.add('document')
                    nested = cell.get('document')
                    require(isinstance(nested, dict) and {'title', 'blocks'} <= set(nested) <= {'title', 'blocks', 'footnotes', 'outline'})
                    if 'outline' in nested:
                        outline = nested['outline']
                        require(isinstance(outline, dict) and set(outline) == {'levels', 'sections'}, 'invalid nested outline')
                        validate_outline({'title': [], 'levels': outline['levels'], 'class': '', 'sections': outline['sections']})
                    validate_footnotes(nested)
                    if nested['title'] is not None:
                        validate_inlines(nested['title'])
                    require(isinstance(nested['blocks'], list))
                    for child in nested['blocks']:
                        validate_block(child)
                elif cell['style'] == 'literal':
                    keys.add('literal')
                    validate_inlines(cell.get('literal'))
                else:
                    keys.add('paragraphs')
                    validate_labels(cell.get('paragraphs'))
                require(set(cell) == keys)


def validate_outline(value):
    if value is None:
        return
    require(isinstance(value, dict) and set(value) == {'title', 'levels', 'class', 'sections'}, 'invalid outline')
    require(isinstance(value['class'], str), 'invalid outline class')
    validate_inlines(value['title'])
    require(type(value['levels']) is int, 'invalid outline levels')
    pending = [value['sections']]
    while pending:
        entries = pending.pop()
        require(isinstance(entries, list), 'invalid outline entries')
        for entry in entries:
            require(isinstance(entry, dict) and set(entry) == {'id', 'inlines', 'sections'}, 'invalid outline entry')
            require(entry['id'] is None or isinstance(entry['id'], str))
            validate_inlines(entry['inlines'])
            pending.append(entry['sections'])


def validate_block(node):
    require(isinstance(node, dict))
    metadata(node)
    keys = {'context', 'id', 'roles', 'title'}
    context = node.get('context')
    if context == 'floating_title':
        require(set(node) == keys | {'level'} and type(node['level']) is int and node['level'] >= 0)
        validate_inlines(node['title'])
        return
    if context in {'thematic_break', 'page_break'}:
        require(set(node) == keys)
        if node['title'] is not None:
            validate_inlines(node['title'])
        return
    require(context in CONTAINERS | LISTS | VERBATIM | {'paragraph', 'image', 'table', 'stem', 'verse', 'toc'})
    if node['title'] is not None:
        validate_inlines(node['title'])
    if context == 'section':
        keys.add('level')
        require(type(node.get('level')) is int and node['level'] >= 0)
        require(node['title'] is not None)
    if context == 'admonition':
        keys.add('type')
        require(node.get('type') in {'note', 'tip', 'important', 'caution', 'warning'})
    if context in QUOTES:
        keys.update({'attribution', 'citetitle'})
        for key in ['attribution', 'citetitle']:
            if node.get(key) is not None:
                validate_inlines(node[key])
    if context in {'toc', 'preamble'}:
        keys.add('outline')
        validate_outline(node.get('outline'))
    if context == 'toc':
        pass
    elif context == 'verse':
        keys.add('inlines')
        validate_inlines(node.get('inlines'))
    elif context in LISTS:
        keys.add('items')
        require(isinstance(node.get('items'), list))
        if context != 'dlist':
            keys.add('style')
            require(node.get('style') is None or isinstance(node['style'], str))
        for entry in node['items']:
            if context == 'dlist':
                require(isinstance(entry, dict) and set(entry) == {'terms', 'description'})
                validate_labels(entry['terms'])
                if entry['description'] is not None:
                    validate_item(entry['description'])
            else:
                validate_item(entry, callout=context == 'colist')
    elif context == 'table':
        keys.update({'columns', 'head', 'body', 'foot'} | TABLE_FIELDS)
        validate_table(node)
    elif context == 'image':
        keys.update({'target', 'src', 'alt', 'attributes', 'svg'})
        validate_image(node)
    elif context == 'stem':
        keys.update({'style', 'inlines'})
        require(node.get('style') in {'asciimath', 'latexmath'})
        validate_inlines(node.get('inlines'))
    elif context in VERBATIM:
        keys.update({'language', 'inlines'})
        require(node.get('language') is None or isinstance(node['language'], str))
        validate_inlines(node.get('inlines'))
    elif context == 'paragraph' or 'inlines' in node:
        require(context not in {'section', 'preamble'})
        keys.add('inlines')
        validate_inlines(node['inlines'])
    else:
        keys.add('blocks')
        require(isinstance(node.get('blocks'), list))
        for child in node['blocks']:
            validate_block(child)
    require(set(node) == keys)


def validate(result, names=None):
    require(isinstance(result, dict) and {'attributes', 'title', 'outline', 'blocks'} <= set(result)
            <= {'attributes', 'title', 'outline', 'blocks', 'footnotes'})
    validate_outline(result['outline'])
    validate_footnotes(result)
    require(isinstance(result['attributes'], dict) and set(result['attributes']) == {'header', 'final'})
    for snapshot in result['attributes'].values():
        require(isinstance(snapshot, dict) and all(isinstance(key, str) for key in snapshot))
        if names is not None:
            require(set(snapshot) == set(names), 'semantic attribute selection mismatch')
        for nodes in snapshot.values():
            if nodes is not None:
                validate_inlines(nodes)
    require(set(result['attributes']['header']) == set(result['attributes']['final']))
    if result['title'] is not None:
        validate_inlines(result['title'])
    require(isinstance(result['blocks'], list))
    for block in result['blocks']:
        validate_block(block)


def validate_case(case):
    names = case.get('semantic_attributes')
    require(isinstance(names, list) and len(names) == len(set(names)) and all(
        isinstance(name, str) and re.fullmatch(r'[a-z0-9_][a-z0-9_-]*', name) for name in names),
        'invalid semantic attribute selection')
    for value in [case['source'], *case.get('resources', {}).values(), *case.get('attributes', {}).values()]:
        require(not any(char in value for char in '\ue000\ue001'), 'reserved semantic handle character in input')
