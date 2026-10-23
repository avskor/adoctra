#!/usr/bin/env python3
"""Reproducible native timings and raw Cargo WASM size, without speed thresholds.

No network resources, HTML oracle or third-party Python packages are used.
See docs/performance.adoc for timed boundaries and interpretation.
"""
import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import zlib

import acceptance

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / 'compatibility/acceptance/semantics.json'
OPERATIONS = ('parse', 'tokens', 'json', 'parse-json')
SIZES = (32, 128, 512)
FAMILIES = ('prose', 'references', 'tables', 'incomplete')
SCHEMA = 'adoctra-performance-v1'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def synthetic(family, size):
    """Deterministic source, with no clock, randomness or external resources."""
    if family == 'prose':
        return '= Prose\n\n' + ''.join(
            f'Paragraph {i}: Ж🙂 *bold* _emphasis_ `code` and https://example.org[link].\n\n'
            for i in range(size))
    if family == 'references':
        return '= References\n:word: Value\n\n' + ''.join(
            f'[#s{i}]\n== Section {i}\n\n{{word}} footnote:n{i}[Note *{i}*]. '
            f'See <<s{(i + 1) % size}>> and footnote:n{i}[].\n\n' for i in range(size))
    if family == 'tables':
        return '[cols="1,1,a",options=noheader]\n|===\n' + ''.join(
            f'|Row {i} |*Value {i}* |Cell {i}.\n\n* First\n* Second\n\n'
            for i in range(size)) + '|===\n'
    if family == 'incomplete':
        # Real editor-like unfinished constructs, not an unbounded nesting test.
        return ''.join(f'Ж{i} *unfinished link:https://example.org[open\n\n' for i in range(size)) + '\n----\nunfinished listing\n'
    raise ValueError(f'unknown workload family: {family}')


def workloads(manifest=MANIFEST, sizes=SIZES):
    metadata, loaded = acceptance.load_corpus(manifest)  # Validates bytes, provenance and licenses.
    original = {entry['id']: entry for entry in metadata['cases']}
    result = []
    for case in loaded:
        entry = original[case['id']]
        paths = [entry['source'], *entry.get('resources', {}).values()]
        result.append({
            'id': case['id'].removeprefix('semantic-'), 'kind': 'document',
            'source': case['source'], 'resources': case.get('resources', {}),
            'attributes': case.get('attributes', {}),
            'safe_mode': 'safe' if 'resources' in case else 'secure',
            'provenance': {'manifest': str(manifest.relative_to(ROOT)), 'case': case['id'],
                           'files': {path: metadata['files'][path] for path in paths},
                           'origins': metadata['origins']},
        })
    for family in FAMILIES:
        for size in sizes:
            result.append({'id': f'stress-{family}-{size}', 'kind': 'synthetic',
                           'source': synthetic(family, size), 'resources': {}, 'attributes': {},
                           'safe_mode': 'secure', 'provenance': {'generator': 'synthetic-v1', 'family': family, 'units': size}})
    return result


def summary(elapsed, iterations):
    if (not isinstance(elapsed, list) or not elapsed
            or any(type(value) is not int or value <= 0 for value in elapsed)
            or type(iterations) is not int or iterations <= 0):
        raise ValueError('invalid timing samples')
    per_operation = [value / iterations for value in elapsed]
    median = statistics.median(per_operation)
    return {'median_ns': median, 'min_ns': min(per_operation), 'max_ns': max(per_operation),
            'mad_ns': statistics.median(abs(value - median) for value in per_operation)}


def validate_driver(report, case, settings):
    required = {'schema', 'debug_assertions', 'source_bytes', 'resource_bytes', 'settings', 'observations', 'elapsed_ns'}
    if (not isinstance(report, dict) or set(report) != required
            or report['schema'] != 'adoctra-benchmark-driver-v1' or report['debug_assertions'] is not False
            or not isinstance(report['settings'], dict)
            or any(type(value) is not int for value in report['settings'].values())
            or report['settings'] != settings):
        raise ValueError('invalid driver schema, settings or non-release build')
    if (type(report['source_bytes']) is not int or type(report['resource_bytes']) is not int
            or report['source_bytes'] != len(case['source'].encode())
            or report['resource_bytes'] != sum(len(value.encode()) for value in case['resources'].values())):
        raise ValueError('driver input size mismatch')
    observations = report['observations']
    if (not isinstance(observations, dict) or set(observations) != {'root_blocks', 'tokens', 'json_bytes', 'diagnostics'}
            or any(type(observations[key]) is not int or observations[key] < 0 for key in ['root_blocks', 'tokens', 'json_bytes'])
            or observations['json_bytes'] == 0):
        raise ValueError('invalid driver observations')
    diagnostics = observations['diagnostics']
    if (not isinstance(diagnostics, dict) or set(diagnostics) != {'info', 'warning', 'error'}
            or any(type(value) is not int or value < 0 for value in diagnostics.values())):
        raise ValueError('invalid diagnostics')
    # A missing include or broken input must not silently become a faster benchmark.
    incomplete = case['provenance'].get('family') == 'incomplete'
    if not incomplete and (diagnostics['warning'] or diagnostics['error']):
        raise ValueError(f"unexpected diagnostics in {case['id']}: {diagnostics}")
    if not isinstance(report['elapsed_ns'], dict) or set(report['elapsed_ns']) != set(OPERATIONS):
        raise ValueError('missing or extra benchmark operation')
    for operation in OPERATIONS:
        samples = report['elapsed_ns'][operation]
        if not isinstance(samples, list) or len(samples) != settings['samples']:
            raise ValueError('wrong sample count')
        summary(samples, settings['iterations'])


def invoke(command, **kwargs):
    return subprocess.run(command, cwd=ROOT, text=True, encoding='utf-8', capture_output=True, check=True, **kwargs)


def artifact(output, target_name, suffix=None):
    found = []
    for line in output.splitlines():
        message = json.loads(line)
        if message.get('reason') != 'compiler-artifact' or message['target']['name'] != target_name:
            continue
        if suffix is None:
            if message.get('executable'):
                found.append(Path(message['executable']))
        else:
            found.extend(Path(name) for name in message['filenames'] if name.endswith(suffix))
    if len(found) != 1 or not found[0].is_file():
        raise ValueError(f'expected one built artifact for {target_name}, got {found}')
    return found[0]


def build(wasm=False):
    command = ['cargo', 'build', '--locked', '--release', '--message-format=json-render-diagnostics']
    command += (['-p', 'adoctra-wasm', '--target', 'wasm32-unknown-unknown'] if wasm
                else ['-p', 'adoctra-json', '--example', 'performance'])
    print('Building ' + ('WASM' if wasm else 'native driver') + '…', file=sys.stderr, flush=True)
    result = invoke(command, timeout=900)
    if result.stderr:
        print(result.stderr, file=sys.stderr, end='')
    return artifact(result.stdout, 'adoctra_wasm' if wasm else 'performance', '.wasm' if wasm else None), command


def wasm_size(path, command):
    data = path.read_bytes()
    if data[:8] != b'\0asm\x01\0\0\0':
        raise ValueError('artifact is not a WASM v1 module')
    return {'status': 'measured', 'stage': 'cargo-cdylib-before-wasm-bindgen', 'command': command,
            'sha256': digest(data), 'bytes': len(data),
            'gzip_bytes': len(gzip.compress(data, compresslevel=9, mtime=0)),
            'gzip_level': 9, 'gzip_mtime': 0, 'zlib_version': zlib.ZLIB_RUNTIME_VERSION}


def run_case(binary, case, settings, timeout):
    with tempfile.TemporaryDirectory(prefix='adoctra-perf-') as directory:
        root = Path(directory)
        source = root / 'source.adoc'
        source.write_bytes(case['source'].encode())
        command = [str(binary), '--source', str(source), '--safe-mode', case['safe_mode']]
        for name, value in settings.items():
            command.extend(['--' + name, str(value)])
        for name, value in sorted(case['attributes'].items()):
            command.extend(['--attribute', name, value])
        for index, (name, content) in enumerate(sorted(case['resources'].items())):
            path = root / f'resource-{index}'
            path.write_bytes(content.encode())
            command.extend(['--resource', name, str(path)])
        output = invoke(command, timeout=timeout)
    if output.stderr.strip():
        raise ValueError(f"unexpected driver stderr for {case['id']}: {output.stderr}")
    report = json.loads(output.stdout)
    validate_driver(report, case, settings)
    return {'id': case['id'], 'kind': case['kind'], 'provenance': case['provenance'],
            'input': {'source_bytes': report['source_bytes'], 'source_sha256': digest(case['source'].encode()),
                      'resource_bytes': report['resource_bytes'],
                      'resources': {name: {'bytes': len(value.encode()), 'sha256': digest(value.encode())}
                                    for name, value in sorted(case['resources'].items())},
                      'attributes': case['attributes'], 'safe_mode': case['safe_mode'], 'embedded': True,
                      'virtual_source': '/main.adoc', 'showtitle': True},
            'observations': report['observations'],
            'measurements': {operation: {'elapsed_ns': report['elapsed_ns'][operation],
                                        **summary(report['elapsed_ns'][operation], settings['iterations'])}
                             for operation in OPERATIONS}}


def fingerprint():
    paths = [ROOT / name for name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'scripts/performance.py']]
    paths += [p for p in [ROOT / '.cargo/config', ROOT / '.cargo/config.toml'] if p.is_file()]
    paths += [p for p in (ROOT / 'crates').rglob('*') if p.is_file() and p.suffix in {'.rs', '.toml'}]
    files = {str(path.relative_to(ROOT)): digest(path.read_bytes()) for path in sorted(paths)}
    return digest(json.dumps(files, sort_keys=True).encode())


def environment():
    cpu_model = None
    cpuinfo = Path('/proc/cpuinfo')
    if cpuinfo.exists():
        cpu_model = next((line.partition(':')[2].strip() for line in cpuinfo.read_text().splitlines()
                          if line.startswith('model name')), None)
    return {'system': platform.system(), 'release': platform.release(), 'machine': platform.machine(),
            'cpu_model': cpu_model, 'logical_cpus': os.cpu_count(),
            'affinity': sorted(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') else None,
            'load_average': list(os.getloadavg()) if hasattr(os, 'getloadavg') else None,
            'python': platform.python_version(), 'rustc': invoke(['rustc', '-Vv']).stdout.strip(),
            'cargo': invoke(['cargo', '-V']).stdout.strip(),
            'build_environment': {name: value for name, value in sorted(os.environ.items())
                                  if name.startswith('CARGO_PROFILE_RELEASE_') or name in {
                                      'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
                                      'CARGO_BUILD_TARGET', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER'}}}


def write_report(path, report):
    path.parent.mkdir(parents=True, exist_ok=True)
    # Atomic replacement: failure never leaves a partial report at the destination.
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            json.dump(report, stream, ensure_ascii=False, indent=2, allow_nan=False)
            stream.write('\n')
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
    try:
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def bounded_count(value):
    number = int(value)
    if not 0 <= number <= 10_000:
        raise argparse.ArgumentTypeError('must be between 0 and 10000')
    return number


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/performance/report.json')
    parser.add_argument('--case', action='append', default=[], help='exact case id; repeatable')
    parser.add_argument('--smoke', action='store_true', help='two real pages and the smallest stress inputs')
    parser.add_argument('--samples', type=bounded_count, default=5)
    parser.add_argument('--iterations', type=bounded_count, default=3)
    parser.add_argument('--warmup', type=bounded_count, default=2)
    parser.add_argument('--timeout', type=float, default=180, help='per-case process timeout in seconds')
    parser.add_argument('--skip-wasm', action='store_true', help='record WASM size as explicitly skipped')
    args = parser.parse_args(argv)
    if not args.samples or not args.iterations or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error('samples, iterations and timeout must be positive')
    if args.smoke and args.case:
        parser.error('--smoke and --case are mutually exclusive')
    cases = workloads()
    if args.case:
        missing = set(args.case) - {case['id'] for case in cases}
        if missing:
            parser.error(f'unknown cases: {sorted(missing)}')
        cases = [case for case in cases if case['id'] in args.case]
    elif args.smoke:
        cases = [case for case in cases if case['id'] in {'asciidoctor-docs-custom-converter', 'spring-webtestclient'}
                 or case['kind'] == 'synthetic' and case['provenance']['units'] == SIZES[0]]
    code = fingerprint()
    revision = invoke(['git', 'rev-parse', 'HEAD']).stdout.strip()
    dirty = bool(invoke(['git', 'status', '--porcelain']).stdout)
    native, native_command = build()
    wasm = {'status': 'skipped'}
    if not args.skip_wasm:
        wasm_path, wasm_command = build(wasm=True)
        wasm = wasm_size(wasm_path, wasm_command)
    settings = {'warmup': args.warmup, 'samples': args.samples, 'iterations': args.iterations}
    report = {'schema': SCHEMA, 'started_utc': datetime.now(timezone.utc).isoformat(),
              'revision': revision, 'dirty': dirty, 'code_sha256': code,
              'environment': environment(), 'settings': settings, 'operation_order': list(OPERATIONS),
              'native': {'command': native_command, 'sha256': digest(native.read_bytes())}, 'wasm': wasm, 'cases': []}
    for case in cases:
        result = run_case(native, case, settings, args.timeout)
        report['cases'].append(result)
        timing = ' '.join(f"{operation}={result['measurements'][operation]['median_ns'] / 1e6:.3f}ms" for operation in OPERATIONS)
        print(f"{case['id']}: {timing}", file=sys.stderr, flush=True)
    if fingerprint() != code:
        raise ValueError('source changed while measuring; report not written')
    report['finished_utc'] = datetime.now(timezone.utc).isoformat()
    write_report(args.output, report)
    print(f'Report: {args.output}')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f'performance: {error}', file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr, file=sys.stderr)
        sys.exit(1)
