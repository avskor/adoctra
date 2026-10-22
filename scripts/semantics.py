"""inline-semantics-v1: bounded semantic trees, independent of HTML rendering.

No HTML parser is used. Ruby emits nodes through a custom converter; adoctra
exports its resolved JSON AST. Unsupported nodes/raw markup fail closed.
"""
import copy
import html
import re

PROFILE = 'inline-semantics-v1'
FORMATS = {'strong', 'emphasis', 'monospace', 'mark', 'superscript', 'subscript', 'span'}
CONTAINERS = {'section', 'preamble', 'open'}


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
        if 'inlines' in node:
            node['inlines'] = canonical(node['inlines'], prose)
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
                allowed = {'kind', 'target', 'role'}
                if kind == 'open-xref':
                    allowed.update({'refid', 'path', 'fragment'})
                require(set(part) <= allowed and part.get('role') in {None, 'bare'}, 'link metadata is outside inline-semantics-v1')
                node = {'kind': 'xref' if kind == 'open-xref' else 'link', 'target': lexical(part['target']), 'inlines': []}
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
            require(not node.get('role'), 'xref role is outside inline-semantics-v1')
            stack[-1][1].append({'kind': name, 'target': node['target'], 'inlines': inline_projection(node['inlines'])})
        elif name == 'anchor':
            stack[-1][1].append({'kind': name, 'id': node['id']})
        else:
            raise ValueError(f'unsupported semantic inline: {name}')
    require(len(stack) == 1, 'unbalanced semantic parts')
    return result


def block_projection(block):
    context = block['name']
    require(context in CONTAINERS | {'paragraph'}, f'unsupported semantic block: {context}')
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
    if context == 'paragraph':
        result['inlines'] = canonical(inline_projection(block['inlines']))
    else:
        require(not block.get('simple', False), 'simple containers are outside inline-semantics-v1')
        result['blocks'] = [block_projection(child) for child in children]
    return result


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
    result = {'attributes': attributes, 'title': title, 'blocks': [block_projection(block) for block in blocks]}
    validate(result, names)
    return result


def reference_projection(result, names):
    """Decode Ruby's lexical substitution strings, never parse rendered HTML."""
    result = copy.deepcopy(result)

    def inlines(nodes, prose=True):
        for node in nodes:
            if node['kind'] == 'text':
                node['value'] = lexical(node['value'])
            for key in ['id', 'target']:
                if node.get(key) is not None:
                    node[key] = lexical(node[key])
            if 'roles' in node:
                node['roles'] = [lexical(role) for role in node['roles']]
            if 'inlines' in node:
                node['inlines'] = inlines(node['inlines'], prose)
        return canonical(nodes, prose)

    def block(node):
        if node['id'] is not None:
            node['id'] = lexical(node['id'])
        node['roles'] = [lexical(role) for role in node['roles']]
        if node['title'] is not None:
            node['title'] = inlines(node['title'])
        if 'inlines' in node:
            node['inlines'] = inlines(node['inlines'])
        for child in node.get('blocks', []):
            block(child)

    # Validate the raw oracle schema before walking it, then canonical output.
    validate(result, names)
    for stage in result['attributes'].values():
        for name, value in stage.items():
            if value is not None:
                stage[name] = inlines(value, prose=False)
    if result['title'] is not None:
        result['title'] = inlines(result['title'])
    for node in result['blocks']:
        block(node)
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
        elif kind == 'anchor':
            keys.add('id')
            require(isinstance(node.get('id'), str))
        else:
            require(kind == 'line-break')
        require(set(node) == keys)
        if 'inlines' in keys:
            validate_inlines(node['inlines'])


def metadata(node):
    require(node.get('id') is None or isinstance(node['id'], str))
    require(isinstance(node.get('roles'), list) and all(isinstance(role, str) for role in node['roles']))


def validate_block(node):
    require(isinstance(node, dict))
    metadata(node)
    keys = {'context', 'id', 'roles', 'title'}
    context = node.get('context')
    require(context in CONTAINERS | {'paragraph'})
    if node['title'] is not None:
        validate_inlines(node['title'])
    if context == 'section':
        keys.add('level')
        require(type(node.get('level')) is int and node['level'] >= 1)
        require(node['title'] is not None)
    if context == 'paragraph':
        keys.add('inlines')
        validate_inlines(node['inlines'])
    else:
        keys.add('blocks')
        require(isinstance(node.get('blocks'), list))
        for child in node['blocks']:
            validate_block(child)
    require(set(node) == keys)


def validate(result, names=None):
    require(isinstance(result, dict) and set(result) == {'attributes', 'title', 'blocks'})
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
