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
            self.send_header('Content-Type', 'text/plain; charset=utf-8')
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
            raise ValueError(f'unrecognized diagnostic: {line!r}')
        name, number, severity, message = match.groups()
        diagnostics.append(dict(severity=severity, message=message,
                                location=dict(file=name, line=int(number))))
    return diagnostics


def run_side(case, cli, reference):
    with tempfile.TemporaryDirectory(prefix='adoctra-uri-') as directory, server(case['responses']) as (uri, trace):
        root = Path(directory)
        source = case['source'].replace('{uri}', uri)
        (root / 'main.adoc').write_text(source, encoding='utf-8')
        for name, text in case.get('files', {}).items():
            (root / name).write_text(text, encoding='utf-8')
        attributes = {'showtitle': ''}
        if case.get('allow', True):
            attributes['allow-uri-read'] = ''
        attributes.update(case.get('attributes', {}))
        safe = case.get('safe', 'safe')
        config = dict(file=str(root / 'main.adoc'), safe=safe, attributes=attributes)
        if reference:
            command = ['ruby', str(ROOT / 'scripts/asciidoctor-uri-reference.rb')]
        else:
            command = [str(cli), config['file'], '--embedded', '--stdout', '-S', safe]
            for name, value in attributes.items():
                command += ['-a', name + '=' + value]
        # Keep local requests independent of the user's proxy configuration.
        env = {k: v for k, v in os.environ.items() if k.lower() not in
               {'http_proxy', 'https_proxy', 'all_proxy'}}
        completed = subprocess.run(command, input=json.dumps(config), capture_output=True,
                                   text=True, encoding='utf-8', timeout=30, env=env)
        if completed.returncode:
            raise RuntimeError(f'{command[0]} failed: {completed.stderr}')
        if reference:
            if completed.stderr:
                raise RuntimeError(f'unexpected reference stderr: {completed.stderr}')
            output = json.loads(completed.stdout)
        else:
            output = dict(html=completed.stdout, diagnostics=cli_diagnostics(completed.stderr))
        output['requests'] = list(trace)
        return canonical(output, root, uri)


def compare(case, expected, actual):
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
    if (projection(expected['html']) != projection(actual['html'])
            or expected['diagnostics'] != actual['diagnostics']):
        return 'DIFF'
    return 'PASS'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, default=ROOT / 'target/debug/adoctra')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--baseline', type=Path,
                        help='also require the complete recorded observations to remain unchanged')
    args = parser.parse_args()
    reports = []
    for case in cases():
        expected = run_side(case, args.cli.resolve(), True)
        actual = run_side(case, args.cli.resolve(), False)
        status = compare(case, expected, actual)
        reports.append(dict(id=case['id'], status=status, input=case,
                            reference=expected, actual=actual))
        print(f'{status} {case["id"]}', flush=True)
    counts = dict(sorted(Counter(item['status'] for item in reports).items()))
    report = dict(profile='uri-cache-body-dom-diagnostics-requests-v1',
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
    return int(any(status not in {'PASS', 'RUBY_ERROR'} for status in counts))


if __name__ == '__main__':
    raise SystemExit(main())
