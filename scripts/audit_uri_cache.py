#!/usr/bin/env python3
"""Compare URI includes against pinned Ruby using only a local HTTP server.

Checks normalized HTML, ordered diagnostics with primary file/line, and the
complete ordered HTTP request trace. Each side gets a fresh response sequence
and process; no external URL or shared disk cache participates in the probes.
Install the gems named in compatibility/{asciidoctor,open-uri-cached}-version.
"""
import argparse
from collections import Counter
from contextlib import contextmanager
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import threading

from compatibility import projection

ROOT = Path(__file__).resolve().parent.parent


def cases():
    first = {'body': '*first*\n'}
    changing = {'/part.adoc': [first, {'body': '*changed*\n'}]}
    include = 'include::{uri}/part.adoc[]\n'
    repeated = include + '\n' + include
    result = []

    def add(identifier, source, responses=changing, requests=None, **kwargs):
        result.append(dict(id=identifier, source=source, responses=responses,
                           requests=requests if requests is not None else ['/part.adoc'], **kwargs))

    add('uncached-changing', repeated, requests=['/part.adoc'] * 2)
    add('host-cache', repeated, attributes={'cache-uri': ''})
    add('header-cache', ':cache-uri:\n\n' + repeated)
    add('false-value-enables', ':cache-uri: false\n\n' + repeated)
    add('sticky-after-unset', ':cache-uri:\n\n' + include + '\n:cache-uri!:\n\n' + include)
    add('late-activation', include + '\n:cache-uri:\n\n' + repeated,
        requests=['/part.adoc'] * 2)
    add('unset-before-read', ':cache-uri:\n:cache-uri!:\n\n' + repeated,
        requests=['/part.adoc'] * 2)
    add('host-unset-lock', ':cache-uri:\n\n' + repeated,
        attributes={'cache-uri!': ''}, requests=['/part.adoc'] * 2)
    add('denied-uri-does-not-read', ':cache-uri:\n\n' + repeated, allow=False, requests=[])
    add('secure-does-not-read', ':cache-uri:\n\n' + repeated, safe='secure', requests=[])
    add('disabled-depth-does-not-read', ':cache-uri:\n\n' + repeated,
        attributes={'max-include-depth': '0'}, requests=[])
    add('server-cache', ':cache-uri:\n\n' + repeated, safe='server')
    add('unsafe-cache', ':cache-uri:\n\n' + repeated, safe='unsafe')
    add('local-include-does-not-activate', ':cache-uri:\n\ninclude::local.adoc[]\n\n'
        ':cache-uri!:\n\n' + repeated, files={'local.adoc': 'local\n'},
        requests=['/part.adoc'] * 2)
    add('relative-and-absolute-share', ':cache-uri:\n\ninclude::{uri}/outer.adoc[]\n\n' + include,
        {'/outer.adoc': [{'body': 'include::part.adoc[]\n'}], **changing},
        ['/outer.adoc', '/part.adoc'])
    add('selection-is-not-cache-key', ':cache-uri:\n\ninclude::{uri}/part.adoc[lines=1]\n\n'
        'include::{uri}/part.adoc[lines=3]\n',
        {'/part.adoc': [{'body': '*one*\n\n*three*\n'}, {'body': 'changed\n'}]})
    add('tag-selection-and-positions', ':cache-uri:\n\ninclude::{uri}/part.adoc[tag=one]\n\n'
        'include::{uri}/part.adoc[tag=two]\n',
        {'/part.adoc': [{'body': '// tag::one[]\n*one*\n// end::one[]\n// end::two[]\n'}]})
    add('nested-error-position', ':cache-uri:\n\ninclude::{uri}/outer.adoc[]\n',
        {'/outer.adoc': [{'body': '\ninclude::missing.adoc[]\n'}]},
        ['/outer.adoc', '/missing.adoc'])
    for status in [404, 403, 500]:
        add(f'retry-{status}', ':cache-uri:\n\n' + repeated,
            {'/part.adoc': [{'status': status}, first]}, ['/part.adoc'] * 2)
        add(f'optional-{status}-still-errors', ':cache-uri:\n\n'
            'include::{uri}/part.adoc[opts=optional]\n',
            {'/part.adoc': [{'status': status}]})
    add('redirect-key', ':cache-uri:\n\n' + repeated + '\ninclude::{uri}/final.adoc[]\n',
        {'/part.adoc': [{'status': 302, 'location': '/final.adoc'}],
         '/final.adoc': [first, {'body': 'direct\n'}]},
        ['/part.adoc', '/final.adoc', '/final.adoc'])
    add('query-key', ':cache-uri:\n\ninclude::{uri}/part.adoc?a=1[]\n\n'
        'include::{uri}/part.adoc?a=2[]\n\ninclude::{uri}/part.adoc?a=1[]\n',
        {'/part.adoc?a=1': [first], '/part.adoc?a=2': [{'body': 'other\n'}]},
        ['/part.adoc?a=1', '/part.adoc?a=2'])
    add('fragment-key', ':cache-uri:\n\ninclude::{uri}/part.adoc#one[]\n\n'
        'include::{uri}/part.adoc#two[]\n\ninclude::{uri}/part.adoc#one[]\n',
        requests=['/part.adoc'] * 2)
    add('ignore-cache-control', ':cache-uri:\n\n' + repeated,
        {'/part.adoc': [{'body': 'first\n', 'headers': {'Cache-Control': 'no-store'}}]})
    add('empty-response', ':cache-uri:\n\n' + repeated,
        {'/part.adoc': [{'body': ''}, first]})
    add('unicode-hit-ruby-error', ':cache-uri:\n\n' + repeated,
        {'/part.adoc': [{'body': '*Привет*\n'}]}, ruby_error='Encoding::UndefinedConversionError')
    add('encoding-ignored-on-hit', ':cache-uri:\n\n' + include + '\n'
        'include::{uri}/part.adoc[encoding=UTF-16LE]\n')
    add('bom-hit', ':cache-uri:\n\n' + repeated,
        {'/part.adoc': [{'hex': (b'\xfe\xff' + '*one*\n'.encode('utf-16be')).hex()}]})
    add('binary-omission-hit', ':cache-uri:\n\ninclude::{uri}/part.adoc[lines=1]\n\n'
        'include::{uri}/part.adoc[lines=3]\n',
        {'/part.adoc': [{'hex': b'one\n\xff\nthree\n'.hex()}]})
    add('encoding-failure-is-cached', ':cache-uri:\n\n'
        'include::{uri}/part.adoc[encoding=US-ASCII]\n\n'
        'include::{uri}/part.adoc[lines=1]\n',
        {'/part.adoc': [{'hex': b'one\n\xff\n'.hex()}]}, ruby_error='ArgumentError')
    # Uncached URI includes decode like files: an explicit `encoding`, a BOM,
    # selection after decoding and an undecodable byte.
    def encoded(text, codec):
        return [{'hex': text.encode(codec).hex()}]
    for identifier, body, attrs in [
            ('windows-1251', encoded('*Привет*\n', 'cp1251'), 'encoding=Windows-1251'),
            ('shift-jis', encoded('*日本語*\n', 'shift_jis'), 'encoding=Shift_JIS'),
            ('iso-8859-1-lines', encoded('one\n*Grüße*\n', 'latin-1'), 'encoding=ISO-8859-1,lines=2'),
            ('utf-16le-bom', [{'hex': (b'\xff\xfe' + '*Привет*\n'.encode('utf-16le')).hex()}], ''),
            ('utf-16be-explicit', encoded('*Привет*\n', 'utf-16be'), 'encoding=UTF-16BE'),
            ('euc-jp-tag', encoded('// tag::t[]\n*日本*\n// end::t[]\nout\n', 'euc_jp'), 'encoding=EUC-JP,tag=t'),
            ('invalid-utf8-lines', [{'hex': b'one\n\xff\nthree\n'.hex()}], 'lines=3')]:
        add(f'uncached-encoding-{identifier}', f'include::{{uri}}/part.adoc[{attrs}]\n',
            {'/part.adoc': body})
    # Like a file include, a selected undecodable line aborts Ruby's load.
    add('uncached-encoding-invalid-utf8', 'include::{uri}/part.adoc[]\n',
        {'/part.adoc': [{'hex': b'one\n\xff\nthree\n'.hex()}]}, ruby_error='ArgumentError')
    # OpenURI tags the response with `encoding` without transcoding it: lines
    # split in that encoding and only selected lines are converted, so an
    # unselected undecodable line never fails (a file read would).
    def raw(identifier, data, attrs, **kwargs):
        add(f'raw-{identifier}', f'include::{{uri}}/part.adoc[{attrs}]\n',
            {'/part.adoc': [{'hex': data.hex(), **kwargs.pop('response', {})}]}, **kwargs)
    raw('unselected-invalid-us-ascii', b'one\n\xff\n', 'encoding=US-ASCII,lines=1')
    raw('unselected-unmapped-windows-1251', b'*one*\n\x98\n*three*\n', 'encoding=Windows-1251,lines=1;3')
    raw('unselected-invalid-shift-jis-tags', b'// tag::t[]\n*\x93\xfa*\n// end::t[]\n\x81\n',
        'encoding=Shift_JIS,tag=t')
    # A single-byte line is always valid for the marker regexp, even unmapped.
    raw('unmapped-marker-line-windows-1251', b'// tag::t[] \x98\n*in*\n// end::t[]\nout\n',
        'encoding=Windows-1251,tag=t')
    # The marker regexp raises on a malformed multibyte line: not readable.
    raw('malformed-marker-line-shift-jis', b'// tag::t[] \x81\n*in*\n// end::t[]\n',
        'encoding=Shift_JIS,tag=t')
    # `include? '::'` raises for an ASCII-incompatible encoding.
    raw('tags-utf-16le-not-readable', '// tag::t[]\n*in*\n// end::t[]\n'.encode('utf-16le'),
        'encoding=UTF-16LE,tag=t')
    raw('tags-iso-2022-jp-not-readable', '// tag::t[]\n*日本*\n// end::t[]\n'.encode('iso2022_jp'),
        'encoding=ISO-2022-JP,tag=t')
    # A non-ASCII tag name in another encoding never equals the UTF-8 name.
    raw('non-ascii-tag-name-windows-1251', '// tag::тег[]\n*in*\n// end::тег[]\n'.encode('cp1251'),
        'encoding=Windows-1251,tag=тег')
    raw('non-ascii-tag-name-utf-8', '// tag::тег[]\n*in*\n// end::тег[]\n'.encode(), 'tag=тег')
    # The BOM check applies to the first selected line only.
    later_bom = b'skip\n\xef\xbb\xbf' + '*Привет*\n'.encode('cp1251')
    raw('utf-8-bom-first-selected-windows-1251', later_bom, 'encoding=Windows-1251,lines=2')
    raw('utf-8-bom-later-selected-windows-1251', later_bom, 'encoding=Windows-1251,lines=1;2')
    raw('utf-16le-bom-first-selected-windows-1251', b'skip\n\xff\xfe' + '*Привет*'.encode('utf-16le'),
        'encoding=Windows-1251,lines=2')
    raw('utf-16be-bom-whole-windows-1251', b'\xfe\xff' + '*Привет*\n'.encode('utf-16be'),
        'encoding=Windows-1251')
    # Stateful ISO-2022-JP converts each selected line on its own.
    raw('iso-2022-jp-lines', '*日本*\n*語*\n'.encode('iso2022_jp'), 'encoding=ISO-2022-JP,lines=2')
    # The dummy UTF-16 converter consumes its BOM; a second one is content.
    raw('utf-16-double-bom', b'\xfe\xff\xfe\xff' + '*a*\n'.encode('utf-16be'), 'encoding=UTF-16')
    # IBM037 splits at byte 0x0A, so its encoded LF (0x25) stays in the line.
    raw('ibm037-line-keeps-lf', '*a*\nb\n'.encode('cp037') + b'\x0a' + 'c'.encode('cp037'),
        'encoding=IBM037,lines=1')
    raw('unknown-encoding-is-utf-8', '*Привет*\n'.encode(), 'encoding=bogus,lines=1')
    # The Content-Type charset never applies: `encoding` (default UTF-8) is set.
    raw('charset-ignored', '*Привет*\n'.encode(),
        '', response={'content_type': 'text/plain; charset=windows-1251'})
    raw('charset-not-used-for-decoding', b'one\n' + '*Привет*\n'.encode('cp1251'),
        'lines=1', response={'content_type': 'text/plain; charset=windows-1251'})
    # Up to 10240 bytes OpenURI keeps a StringIO, whose `each_line` finds a
    # multi-byte LF at any byte offset; a larger body is a Tempfile, whose IO
    # matches whole code units only. U+0100 U+0A41 U+4100 U+0A42 U+000A holds
    # `00 0A` at bytes 1 and 5 in UTF-16BE: the StringIO's second line is the
    # valid shifted pair `41 41 00 0A`, its odd first line fails conversion.
    shifted = ''.join(map(chr, [0x100, 0xa41, 0x4100, 0xa42, 0xa])).encode('utf-16be')
    raw('small-utf-16be-unaligned-line', shifted, 'encoding=UTF-16BE,lines=2')
    raw('small-utf-16be-unaligned-odd-line', shifted, 'encoding=UTF-16BE,lines=1',
        ruby_error='ArgumentError')
    # U+0A41 U+4E00 holds `0A 00 00 00` at byte 1 in UTF-32LE.
    raw('small-utf-32le-unaligned-odd-line', '*\u0a41\u4e00*\n'.encode('utf-32le'),
        'encoding=UTF-32LE,lines=1', ruby_error='ArgumentError')
    padding = 'x\n' * 2600
    for codec, name, text in [('utf-16be', 'UTF-16BE', '*\u0100\u0a41*\n*two*\n'),
                              ('utf-32le', 'UTF-32LE', '*\u0a41\u4e00*\n*two*\n')]:
        for lines in ['1', '2', '1;2']:
            raw(f'large-{codec}-lines={lines}', (text + padding).encode(codec),
                f'encoding={name},lines={lines}')
    # open-uri-cached patches OpenURI for the whole Ruby process: once a
    # document enables `cache-uri`, later inputs of the same run read through
    # the cache too, even without (or with an unset) `cache-uri`.
    add('run-later-input-hits', ':cache-uri:\n\n' + include,
        documents=[include, ':cache-uri!:\n\n' + include])
    add('run-later-input-enables', include, requests=['/part.adoc'] * 2,
        documents=[':cache-uri:\n\n' + include, include])
    add('run-uncached-inputs', include, requests=['/part.adoc'] * 2, documents=[include])
    add('run-hit-is-binary-in-later-input', ':cache-uri:\n\n' + include,
        {'/part.adoc': [{'hex': (b'\xfe\xff\xfe\xff' + '*a*\n'.encode('utf-16be')).hex()}]},
        documents=['include::{uri}/part.adoc[encoding=UTF-16BE]\n'])
    # A cache hit reads as BINARY, so a second UTF-16 BOM stays U+FEFF.
    add('double-bom-hit', ':cache-uri:\n\n' + repeated,
        {'/part.adoc': [{'hex': (b'\xfe\xff\xfe\xff' + '*a*\n'.encode('utf-16be')).hex()}]})
    # Every safe mode for uncached and cached URI includes; secure never reads.
    for safe in ['safe', 'server', 'unsafe', 'secure']:
        reads = safe != 'secure'
        add(f'mode-{safe}-uncached', repeated, safe=safe,
            requests=['/part.adoc'] * 2 if reads else [])
        add(f'mode-{safe}-cached-header', ':cache-uri:\n\n' + repeated, safe=safe,
            requests=['/part.adoc'] if reads else [])
        # Known difference: from server mode up Ruby trims `docfile` to a
        # path relative to `docdir`, so its logger cursor's `file` is
        # `main.adoc`; adoctra's CLI keeps the full path (docs/cli.adoc).
        add(f'mode-{safe}-missing', 'include::{uri}/missing.adoc[]\n', safe=safe,
            requests=['/missing.adoc'] if reads else [],
            **({'known_difference': True} if safe == 'server' else {}))
    return result


def asset_cases():
    image = 'image::{uri}/a.png[Pic]\n'
    svg = 'image::{uri}/a.svg[Pic,opts=inline]\n'
    png = {'/a.png': [{'hex': '89504e4700ff', 'content_type': 'image/png'},
                     {'body': 'changed', 'content_type': 'image/jpeg'}]}
    vector = {'/a.svg': [{'body': '<svg xmlns="http://www.w3.org/2000/svg"><text>one</text></svg>\n',
                          'content_type': 'image/svg+xml'}, {'body': '<svg>changed</svg>'}]}
    result = []

    def add(identifier, source, responses, requests, **kwargs):
        result.append(dict(id=identifier, source=source, responses=responses,
                           requests=requests, **kwargs))

    for cached in [False, True]:
        header = (':cache-uri:\n' if cached else '') + ':data-uri:\n\n'
        suffix = 'cached' if cached else 'uncached'
        add('image-' + suffix, header + image + '\n' + image, png, ['/a.png'] * (1 if cached else 2))
        add('svg-' + suffix, header + svg + '\n' + svg, vector, ['/a.svg'] * (1 if cached else 2))
    add('image-inline-block-share', ':cache-uri:\n:data-uri:\n\n' + image + '\nimage:{uri}/a.png[Pic]\n', png, ['/a.png'])
    add('image-sticky-after-unset', ':cache-uri:\n:data-uri:\n\n' + image + '\n:cache-uri!:\n\n' + image, png, ['/a.png'])
    add('image-late-activation', ':data-uri:\n\n' + image + '\n:cache-uri:\n\n' + image + '\n' + image,
        png, ['/a.png'] * 2)
    add('image-false-enables', ':cache-uri: false\n:data-uri:\n\n' + image + '\n' + image, png, ['/a.png'])
    add('image-imagesdir', ':cache-uri:\n:data-uri:\n:imagesdir: {uri}\n\nimage::a.png[Pic]\n\n' + image, png, ['/a.png'])
    add('image-no-data-uri', ':cache-uri:\n\n' + image + '\n' + image, png, [])
    add('image-denied', ':cache-uri:\n:data-uri:\n\n' + image, png, [], allow=False)
    add('image-secure', ':cache-uri:\n:data-uri:\n\n' + image, png, [], safe='secure')
    add('image-server', ':cache-uri:\n:data-uri:\n\n' + image + '\n' + image, png, ['/a.png'], safe='server')
    add('image-retry', ':cache-uri:\n:data-uri:\n\n' + image + '\n' + image,
        {'/a.png': [{'status': 404}, png['/a.png'][0]]}, ['/a.png'] * 2)
    add('image-error-activates', ':cache-uri:\n:data-uri:\n\nimage::{uri}/missing.png[Missing]\n\n'
        ':cache-uri!:\n\n' + image + '\n' + image, png, ['/missing.png', '/a.png'])
    add('image-empty', ':cache-uri:\n:data-uri:\n\n' + image + '\n' + image,
        {'/a.png': [{'body': '', 'content_type': 'image/png'}]}, ['/a.png'])
    add('image-utf8-bytes', ':cache-uri:\n:data-uri:\n\n' + image + '\n' + image,
        {'/a.png': [{'body': 'Привет', 'content_type': 'application/x-test'}]}, ['/a.png'])
    add('include-before-image-share', ':cache-uri:\n:data-uri:\n\nimage::{uri}/shared.adoc[Pic]\n\n'
        'include::{uri}/shared.adoc[]\n', {'/shared.adoc': [{'body': 'first\n', 'content_type': 'text/plain'}, {'body': 'changed\n'}]}, ['/shared.adoc'])
    add('include-before-image-activation', ':data-uri:\n\n:cache-uri:\n\nimage::{uri}/shared.adoc[Pic]\n\n'
        ':cache-uri!:\n\ninclude::{uri}/shared.adoc[]\n',
        {'/shared.adoc': [{'body': 'first\n'}, {'body': 'changed\n'}]}, ['/shared.adoc'] * 2)
    add('svg-imagesdir', ':cache-uri:\n:imagesdir: {uri}\n\nimage::a.svg[Pic,opts=inline]\n\n' + svg, vector, ['/a.svg'])
    add('svg-denied', ':cache-uri:\n\n' + svg, vector, [], allow=False)
    add('svg-secure', ':cache-uri:\n\n' + svg, vector, [], safe='secure')
    add('svg-retry', ':cache-uri:\n\n' + svg + '\n' + svg,
        {'/a.svg': [{'status': 403}, vector['/a.svg'][0]]}, ['/a.svg'] * 2)
    add('svg-empty', ':cache-uri:\n\n' + svg + '\n' + svg, {'/a.svg': [{'body': ''}]}, ['/a.svg'])
    add('svg-unicode-hit-warning', ':cache-uri:\n\n' + svg + '\n' + svg,
        {'/a.svg': [{'body': '<svg><text>Привет</text></svg>\n'}]}, ['/a.svg'])
    add('svg-utf16-bom', ':cache-uri:\n\n' + svg + '\n' + svg,
        {'/a.svg': [{'hex': (b'\xff\xfe' + '<svg><text>Привет</text></svg>\r\n'.encode('utf-16le')).hex()}]}, ['/a.svg'])
    add('svg-bad-utf8-warning', ':cache-uri:\n\n' + svg + '\n' + svg,
        {'/a.svg': [{'hex': b'<svg>\xff</svg>\n'.hex()}]}, ['/a.svg'])
    add('svg-data-uri-share', ':cache-uri:\n:data-uri:\n\n' + svg + '\nimage::{uri}/a.svg[Pic]\n', vector, ['/a.svg'])
    add('interactive-svg-fallback', ':cache-uri:\n:data-uri:\n\n'
        'image::{uri}/a.svg[Pic,opts=interactive,fallback={uri}/a.png]\n\n' + image, png, ['/a.png', '/a.svg'])
    css = {'/a.css': [{'body': 'body { color: red; }\n', 'content_type': 'text/css'}, {'body': 'changed\n'}]}
    sheet = ':stylesheet: {uri}/a.css\n:cache-uri:\n'
    add('stylesheet-cache', sheet + '\nx\n', css, ['/a.css'], standalone=True)
    add('stylesheet-denied', sheet + '\nx\n', css, [], standalone=True, allow=False)
    add('stylesheet-secure', sheet + '\nx\n', css, [], standalone=True, safe='secure')
    add('stylesheet-linked', sheet + ':linkcss:\n\nx\n', css, [], standalone=True)
    add('stylesheet-error', sheet + '\nx\n', {'/a.css': [{'status': 500}]}, ['/a.css'], standalone=True)
    add('stylesheet-include-share', sheet + '\ninclude::{uri}/a.css[]\n', css, ['/a.css'], standalone=True)
    add('stylesheet-image-share', sheet + ':data-uri:\n\nimage::{uri}/a.css[Pic]\n', css, ['/a.css'], standalone=True)
    # Ruby's head reads the stylesheet (enabling the cache with a header
    # `cache-uri`) before the body's images, so it is requested and warns first.
    add('stylesheet-before-image', sheet + ':data-uri:\n\n' + image, {**css, **png}, ['/a.css', '/a.png'], standalone=True)
    # Pinned Ruby requests the image again after a body `:cache-uri!:` even
    # though the stylesheet enabled the cache (observed trace).
    add('stylesheet-enables-body-cache', sheet + ':data-uri:\n\n:cache-uri!:\n\n' + image + '\n' + image,
        {**css, **png}, ['/a.css', '/a.png', '/a.png'], standalone=True)
    # Every safe mode for each asset kind; secure reads no URI.
    for safe in ['safe', 'server', 'unsafe', 'secure']:
        reads = safe != 'secure'
        add(f'mode-{safe}-image', ':data-uri:\n\n' + image + '\n' + image, png,
            ['/a.png'] * 2 if reads else [], safe=safe)
        add(f'mode-{safe}-image-cached', ':cache-uri:\n:data-uri:\n\n' + image + '\n' + image, png,
            ['/a.png'] if reads else [], safe=safe)
        add(f'mode-{safe}-image-missing', ':data-uri:\n\nimage::{uri}/missing.png[Missing]\n', png,
            ['/missing.png'] if reads else [], safe=safe)
        add(f'mode-{safe}-svg', svg + '\n' + svg, vector, ['/a.svg'] * 2 if reads else [], safe=safe)
        add(f'mode-{safe}-svg-cached', ':cache-uri:\n\n' + svg + '\n' + svg, vector,
            ['/a.svg'] if reads else [], safe=safe)
        add(f'mode-{safe}-svg-missing', 'image::{uri}/missing.svg[Pic,opts=inline]\n', vector,
            ['/missing.svg'] if reads else [], safe=safe)
        add(f'mode-{safe}-stylesheet', sheet + '\nx\n', css, ['/a.css'] if reads else [],
            standalone=True, safe=safe)
        add(f'mode-{safe}-stylesheet-missing', ':stylesheet: {uri}/missing.css\n\nx\n', css,
            ['/missing.css'] if reads else [], standalone=True, safe=safe)
    # The process-wide cache serves a later input's image too.
    add('run-later-input-image', ':cache-uri:\n:data-uri:\n\n' + image, png, ['/a.png'],
        documents=[':data-uri:\n\n' + image, ':data-uri:\n:cache-uri!:\n\n' + image])
    add('stylesheet-warning-order', sheet + ':data-uri:\n\n' + image,
        {'/a.css': [{'status': 500}], '/a.png': [{'status': 404}]},
        ['/a.css', '/a.png'], standalone=True)
    return result


@contextmanager
def server(responses):
    trace = []
    counts = Counter()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            sequence = responses.get(self.path, [{'status': 404}])
            response = sequence[min(counts[self.path], len(sequence) - 1)]
            counts[self.path] += 1
            body = (bytes.fromhex(response['hex']) if 'hex' in response
                    else response.get('body', '').encode('utf-8'))
            status = response.get('status', 200)
            trace.append({'path': self.path, 'status': status,
                          'sha256': hashlib.sha256(body).hexdigest()})
            self.send_response(status)
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Content-Type', response.get('content_type', 'text/plain; charset=utf-8'))
            self.send_header('Connection', 'close')
            if 'location' in response:
                self.send_header('Location', response['location'])
            for key, value in response.get('headers', {}).items():
                self.send_header(key, value)
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_args):
            pass

    httpd = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    worker = threading.Thread(target=lambda: httpd.serve_forever(poll_interval=0.01))
    worker.start()
    try:
        yield f'http://127.0.0.1:{httpd.server_port}', trace
    finally:
        httpd.shutdown()
        worker.join()
        httpd.server_close()


def canonical(value, root, uri):
    if isinstance(value, str):
        return value.replace(str(root), '<fixture>').replace(uri, 'http://fixture.test')
    if isinstance(value, list):
        return [canonical(item, root, uri) for item in value]
    if isinstance(value, dict):
        return {key: canonical(item, root, uri) for key, item in value.items()}
    return value


def cli_diagnostics(stderr):
    diagnostics = []
    for line in stderr.splitlines():
        match = re.fullmatch(r'adoctra: (.+):([1-9]\d*): (warning|error): (.*)', line)
        if not match:
            match = re.fullmatch(r'adoctra: (warning|error): (.*)', line)
            if not match:
                raise ValueError(f'unrecognized diagnostic: {line!r}')
            diagnostics.append(dict(severity=match[1], message=match[2], location=None))
            continue
        name, number, severity, message = match.groups()
        diagnostics.append(dict(severity=severity, message=message,
                                location=dict(file=name, line=int(number))))
    return diagnostics


def run_side(case, cli, reference):
    with tempfile.TemporaryDirectory(prefix='adoctra-uri-') as directory, server(case['responses']) as (uri, trace):
        root = Path(directory)
        source = case['source'].replace('{uri}', uri)
        (root / 'main.adoc').write_text(source, encoding='utf-8')
        # Further inputs of the same run (one Ruby process, one CLI call).
        later = []
        for index, text in enumerate(case.get('documents', []), 2):
            later.append(str(root / f'main{index}.adoc'))
            Path(later[-1]).write_text(text.replace('{uri}', uri), encoding='utf-8')
        for name, text in case.get('files', {}).items():
            (root / name).write_text(text, encoding='utf-8')
        attributes = {'showtitle': ''}
        if case.get('allow', True):
            attributes['allow-uri-read'] = ''
        attributes.update(case.get('attributes', {}))
        standalone = case.get('standalone', False)
        if standalone:
            attributes['reproducible'] = ''
            attributes['webfonts!'] = ''
        safe = case.get('safe', 'safe')
        config = dict(file=str(root / 'main.adoc'), safe=safe, attributes=attributes, standalone=standalone,
                      **({'then': later} if later else {}))
        if reference:
            command = ['ruby', str(ROOT / 'scripts/asciidoctor-uri-reference.rb')]
        else:
            command = [str(cli), config['file'], *later, '--stdout', '-S', safe]
            if not standalone:
                command.append('--embedded')
            for name, value in attributes.items():
                command += ['-a', name + '=' + value]
        # Keep local requests independent of the user's proxy configuration.
        env = {k: v for k, v in os.environ.items() if k.lower() not in
               {'http_proxy', 'https_proxy', 'all_proxy'}}
        # Bytes, not text mode: universal newlines would turn a lone CR into LF.
        raw = subprocess.run(command, input=json.dumps(config).encode('utf-8'), capture_output=True,
                             timeout=30, env=env)
        completed = subprocess.CompletedProcess(raw.args, raw.returncode, raw.stdout.decode('utf-8'),
                                                raw.stderr.decode('utf-8'))
        if completed.returncode:
            raise RuntimeError(f'{command[0]} failed: {completed.stderr}')
        if reference:
            if completed.stderr:
                raise RuntimeError(f'unexpected reference stderr: {completed.stderr}')
            output = json.loads(completed.stdout)
        else:
            output = dict(html=completed.stdout, diagnostics=cli_diagnostics(completed.stderr))
        output['requests'] = list(trace)
        if standalone and 'html' in output:
            output['styles'] = re.findall(r'<style[^>]*>(.*?)</style>', output['html'], re.S)
            output['stylesheet_links'] = re.findall(r'<link\b[^>]*\brel="stylesheet"[^>]*>', output['html'])
        return canonical(output, root, uri)


def html_projection(case, html):
    result = projection(html)
    if case.get('standalone'):
        # Only the three known document shell tags differ. Keep their ids,
        # contents and every other attribute/node; do not erase body content.
        for node in result:
            if isinstance(node, dict) and (node['tag'], node['attrs']) in [
                    ('header', {'id': 'header'}), ('main', {'id': 'content'}),
                    ('footer', {'id': 'footer'})]:
                node['tag'] = 'div'
    return result


def compare_observations(case, expected, actual):
    # HTTP failures/extra loads must remain visible even for a Ruby exception.
    expected_paths = [item['path'] for item in expected['requests']]
    actual_paths = [item['path'] for item in actual['requests']]
    if expected_paths != case['requests'] or actual_paths != case['requests']:
        return 'DIFF'
    if expected['requests'] != actual['requests']:
        return 'DIFF'
    if 'error' in expected:
        return ('RUBY_ERROR' if case.get('ruby_error') == expected['error']['class']
                else 'UNEXPECTED_RUBY_ERROR')
    if case.get('ruby_error'):
        return 'UNEXPECTED_RUBY_SUCCESS'
    if (html_projection(case, expected['html']) != html_projection(case, actual['html'])
            or expected['diagnostics'] != actual['diagnostics']
            or expected.get('styles') != actual.get('styles')
            or expected.get('stylesheet_links') != actual.get('stylesheet_links')):
        return 'DIFF'
    return 'PASS'


def compare(case, expected, actual):
    status = compare_observations(case, expected, actual)
    if case.get('known_difference') and 'error' not in expected:
        return {'DIFF': 'XFAIL', 'PASS': 'XPASS'}.get(status, status)
    return status


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, default=ROOT / 'target/debug/adoctra')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--assets', action='store_true', help='run the separate asset cache probes')
    parser.add_argument('--baseline', type=Path,
                        help='also require the complete recorded observations to remain unchanged')
    args = parser.parse_args()
    if args.assets and not (args.baseline or args.report):
        parser.error('asset XFAILs require --baseline verification or explicit --report recording')
    reports = []
    for case in asset_cases() if args.assets else cases():
        expected = run_side(case, args.cli.resolve(), True)
        actual = run_side(case, args.cli.resolve(), False)
        status = compare(case, expected, actual)
        reports.append(dict(id=case['id'], status=status, input=case,
                            reference=expected, actual=actual))
        print(f'{status} {case["id"]}', flush=True)
    counts = dict(sorted(Counter(item['status'] for item in reports).items()))
    report = dict(profile=('uri-asset-cache-body-dom-styles-diagnostics-requests-v1' if args.assets
                           else 'uri-cache-body-dom-diagnostics-requests-v1'),
                  asciidoctor=(ROOT / 'compatibility/asciidoctor-version').read_text().strip(),
                  open_uri_cached=(ROOT / 'compatibility/open-uri-cached-version').read_text().strip(),
                  counts=counts, cases=reports)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(counts, sort_keys=True))
    if args.baseline and json.loads(args.baseline.read_text(encoding='utf-8')) != report:
        print('DIFF recorded URI cache observations')
        return 1
    return int(any(status not in {'PASS', 'RUBY_ERROR', 'XFAIL'} for status in counts))


if __name__ == '__main__':
    raise SystemExit(main())
