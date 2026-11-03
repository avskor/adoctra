#!/usr/bin/env python3
"""Compare include decoding/selection order; count Ruby crashes separately from PASS.

Run after building the CLI:
  python3 scripts/audit_include_order.py --report /tmp/include-order.json
With --uri the same bytes are served over local HTTP to a URI include, which
the pinned Ruby reads through OpenURI without transcoding (lines split in the
external encoding, only selected lines converted):
  python3 scripts/audit_include_order.py --uri --report /tmp/include-order-uri.json
The input byte fragments were encoded by the pinned Ruby environment. Results
compare normalized HTML and ordered diagnostics; they do not establish exact HTML
compatibility. An uncaught Ruby exception is never counted as a compatible case.
"""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import subprocess

import acceptance as a
import audit_uri_cache as u
import compatibility as c

ROOT = Path(__file__).resolve().parent.parent


def make_case(identifier, data, encoding, selection, listing=False):
    opts = ','.join(part for part in [f'encoding={encoding}' if encoding else '',
                                     selection, 'opts=optional'] if part)
    source = f'include::part.adoc[{opts}]'
    if listing:  # Verbatim content keeps control characters observable.
        source = f'----\n{source}\n----'
    return dict(id=identifier, category='include', expect='pass',
                profile='include-body-dom-v1', source=source,
                resources={}, binary_resources={'part.adoc': data.hex()})


def build_cases():
    codecs = json.loads((ROOT / 'compatibility/encoding-order/encodings.json').read_text())
    cases = []
    for name, meta in codecs.items():
        fragments = {key: bytes.fromhex(value) for key, value in meta['fragments'].items() if value is not None}
        bom = bytes.fromhex(meta['bom'])
        first = bom + fragments['*ok*\r\n']
        variants = [('valid', first + fragments['\nlast\n'])]
        if meta['bad'] is not None:
            bad = bytes.fromhex(meta['bad'])
            variants += [('tail', first + bad),
                         ('tail-lookahead', first + fragments['A'] + bad),
                         ('middle', first + bad + fragments['\nlast\n']),
                         ('outside-tags', bom + fragments['// tag::body[]\n*ok*\n// end::body[]\n'] + bad)]
        for variant, data in variants:
            for selection in ['', 'lines=1', 'lines=2', 'lines=1;3', 'lines=0',
                              'lines=2..1', 'lines=3..-1', 'tag=body']:
                identifier = f'order-{name.lower()}-{variant}-{selection or "all"}'
                cases.append(make_case(identifier, data, name, selection))
        # Decoded CR, NEL and LS stay inside a line: Ruby splits at LF only.
        for text, control in [('a\rb\nc\n', 'cr'), ('a\u0085b\nc\n', 'nel'), ('a\u2028b\nc\n', 'ls')]:
            if meta['fragments'].get(text) is None:
                continue
            data = bom + bytes.fromhex(meta['fragments'][text])
            for selection in ['', 'lines=1', 'lines=2']:
                identifier = f'control-{name.lower()}-{control}-{selection or "all"}'
                cases.append(make_case(identifier, data, name, selection, listing=True))
    for little, explicit in [(True, 'UTF-16LE'), (False, 'UTF-16BE')]:
        data = (b'\xff\xfe' if little else b'\xfe\xff') + '*one*\n*two*\n*three*\n'.encode(
            'utf-16le' if little else 'utf-16be')
        for encoding in [None, 'UTF-8', 'ASCII-8BIT', 'BINARY', 'unknown-bom-audit']:
            for selection in ['', 'lines=1', 'lines=2', 'lines=3', 'lines=2;3', 'lines=0']:
                identifier = f'bom-{explicit.lower()}-{encoding or "default"}-{selection or "all"}'
                cases.append(make_case(identifier, data, encoding, selection))
        for selection in ['lines=2', 'lines=3', 'lines=2;3']:
            cases.append(make_case(f'bom-{explicit.lower()}-explicit-{selection}',
                                   data, explicit, selection))
    return cases + bom_tag_cases()


def bom_tag_cases():
    cases = []
    for encoding in [None, 'UTF-8', 'BINARY']:
        name = encoding or 'default'
        for little, endian, codec in [(False, 'be', 'utf-16be'), (True, 'le', 'utf-16le')]:
            bom = b'\xff\xfe' if little else b'\xfe\xff'
            tagged = bom + '// tag::body[]\n*shown*\n// end::body[]\n*outside*\n'.encode(codec)
            for selector in ['tag=body', 'tag=!body', 'tags=**', 'tags=*', 'tags=**;!body', 'lines=1;2']:
                cases.append(make_case(f'bom-tags-{name}-{endian}-{selector}', tagged, encoding, selector))
            mixed = b'ignored\n' + bom + '*shown*\n*second*'.encode(codec)
            for selector in ['lines=2', 'lines=2;3']:
                cases.append(make_case(f'bom-mixed-{name}-{endian}-{selector}', mixed, encoding, selector))
            ascii_tags = b'// tag::body[]\n' + bom + '*shown*\n*second*\n'.encode(codec) + b'// end::body[]\n'
            cases.append(make_case(f'bom-ascii-tags-{name}-{endian}', ascii_tags, encoding, 'tag=body'))
            glyph = '\u0a41' if little else '上'
            data = bom + f'first{glyph}second'.encode(codec)
            for selector in ['lines=1', 'lines=1;2']:
                cases.append(make_case(f'bom-framed-{name}-{endian}-{selector}', data, encoding, selector))
            # The first kept BOM takes precedence over a skipped initial hint.
            opposite = b'\xfe\xff' + 'ignored\n'.encode('utf-16be') + bom + '*shown*'.encode(codec)
            cases.append(make_case(f'bom-later-hint-{name}-{endian}', opposite, encoding, 'lines=2'))
        for diagnostic in [False, True]:
            suffix = (b'// end::body[]\n// end::body[]\n// tag::body[]\n'
                      if diagnostic else b'// end::body[]\n')
            data = b'// tag::body[]\n' + b'\xff\xfe' + '*shown*\u0a41'.encode('utf-16le') + suffix
            cases.append(make_case(f'bom-le-framed-tags-{name}-{diagnostic}', data, encoding, 'tag=body'))
            data = b'\xef\xbb\xbf// tag::body[]\n' + b'\xfe\xff' + '*shown*\n*second*\n'.encode('utf-16be') + suffix
            cases.append(make_case(f'bom-utf8-prefix-tags-{name}-{diagnostic}', data, encoding, 'tag=body'))
    for little, endian, codec in [(False, 'be', 'UTF-16BE'), (True, 'le', 'UTF-16LE')]:
        bom = b'\xff\xfe' if little else b'\xfe\xff'
        data = bom + '// tag::body[]\n*shown*\n// end::body[]\n*outside*\n'.encode(codec)
        for selector in ['tag=body', 'tag=!body', 'tags=**', 'tags=*', 'tags=**;!body', 'lines=1;2']:
            cases.append(make_case(f'bom-tags-explicit-{endian}-{selector}', data, codec, selector))
    data = (b'\xef\xbb\xbf\xff// tag::body[]\n' + b'\xfe\xff'
            + '*shown*\n'.encode('utf-16be') + b'// end::other[]\n')
    cases.append(make_case('bom-tags-omitted-prefix-diagnostic', data, 'BINARY', 'tag=body'))
    return cases


def compare(reference, actual, root):
    if actual.returncode:
        raise RuntimeError(f'adoctra failed: {actual.stderr}')
    if reference.returncode:
        return dict(status='RUBY_ERROR',
                    reference_error=a.diagnostic_text(reference.stderr.splitlines()[0], root),
                    adoctra_diagnostics=a.cli_diagnostics(actual.stderr, root))
    expected = a.reference_report(reference, root)
    diagnostics = a.cli_diagnostics(actual.stderr, root)
    if (c.projection(expected['html']) != c.projection(actual.stdout)
            or expected['diagnostics'] != diagnostics):
        return dict(status='DIFF', expected=expected,
                    actual=dict(html=actual.stdout, diagnostics=diagnostics))
    return dict(status='PASS')


def invoke(command, source):
    # Bytes, not text mode: universal newlines would turn a lone CR into LF
    # in the HTML of adoctra, while the reference keeps it escaped in JSON.
    raw = subprocess.run(command, input=source.encode('utf-8'), capture_output=True, timeout=30)
    return subprocess.CompletedProcess(raw.args, raw.returncode, raw.stdout.decode('utf-8'),
                                       raw.stderr.decode('utf-8'))


def uri_case(case):
    """The same include as a URI target; the response carries the bytes."""
    return dict(id=case['id'], requests=['/part.adoc'],
                source=case['source'].replace('include::part.adoc', 'include::{uri}/part.adoc'),
                responses={'/part.adoc': [{'hex': case['binary_resources']['part.adoc']}]})


def compare_uri(case, expected, actual):
    if [item['path'] for item in expected['requests']] != case['requests'] \
            or expected['requests'] != actual['requests']:
        return dict(status='DIFF', expected=expected, actual=actual)
    if 'error' in expected:
        return dict(status='RUBY_ERROR', reference_error=expected['error'],
                    adoctra_diagnostics=actual['diagnostics'])
    if (c.projection(expected['html']) != c.projection(actual['html'])
            or expected['diagnostics'] != actual['diagnostics']):
        return dict(status='DIFF', expected=expected, actual=actual)
    return dict(status='PASS')


def run_uri_case(case, binary):
    case = uri_case(case)
    expected = u.run_side(case, binary.resolve(), True)
    actual = u.run_side(case, binary.resolve(), False)
    return dict(id=case['id'], **compare_uri(case, expected, actual))


def run_case(case, binary):
    with c.case_commands(case, binary) as (reference, actual):
        expected = invoke(reference + ['--report'], case['source'])
        actual = invoke(actual, case['source'])
        result = compare(expected, actual, Path(reference[-1]))
    return dict(id=case['id'], **result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/adoctra')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--jobs', type=int, default=3)
    parser.add_argument('--uri', action='store_true',
                        help='serve each include over local HTTP (OpenURI without transcoding)')
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error('--jobs must be positive')
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        run = run_uri_case if args.uri else run_case
        results = list(pool.map(lambda case: run(case, args.binary), build_cases()))
    counts = dict(Counter(result['status'] for result in results))
    report = dict(asciidoctor_version=(ROOT / 'compatibility/asciidoctor-version').read_text().strip(),
                  profile=('include-body-dom-v1 + ordered diagnostics'
                           + (' + HTTP requests (URI include)' if args.uri else '')),
                  counts=counts, cases=results)
    if args.report:
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(counts, sort_keys=True))
    return int(bool(counts.get('DIFF')))


if __name__ == '__main__':
    raise SystemExit(main())
