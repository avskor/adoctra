import contextlib
import copy
import gzip
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import performance as perf


def case():
    return {'id': 'probe', 'kind': 'document', 'source': 'Ж🙂\r\n',
            'resources': {'example$code.adoc': 'Nested.\r\n'},
            'attributes': {'label': 'a value with spaces'}, 'safe_mode': 'safe', 'provenance': {}}


def driver(workload=None):
    workload = workload or case()
    return {'schema': 'adoctra-benchmark-driver-v1', 'debug_assertions': False,
            'source_bytes': len(workload['source'].encode()),
            'resource_bytes': sum(len(value.encode()) for value in workload['resources'].values()),
            'settings': {'warmup': 0, 'samples': 3, 'iterations': 2},
            'observations': {'root_blocks': 1, 'tokens': 4, 'json_bytes': 100,
                             'diagnostics': {'info': 0, 'warning': 0, 'error': 0}},
            'elapsed_ns': {operation: [20, 40, 60] for operation in perf.OPERATIONS}}


class PerformanceTests(unittest.TestCase):
    def test_statistics_normalize_each_sample_by_iterations(self):
        self.assertEqual(perf.summary([30, 60, 90], 3),
                         {'median_ns': 20, 'min_ns': 10, 'max_ns': 30, 'mad_ns': 10})
        self.assertEqual(perf.summary([90], 3)['mad_ns'], 0)
        for samples, iterations in [([], 1), ([0], 1), ([-1], 1), ([True], 1), ([2.5], 1), ([10], 0), ([10], True)]:
            with self.subTest(samples=samples, iterations=iterations), self.assertRaises(ValueError):
                perf.summary(samples, iterations)

    def test_real_workloads_reuse_every_pinned_page_resource_and_attribute(self):
        metadata, original = perf.acceptance.load_corpus(perf.MANIFEST)
        workloads = perf.workloads()
        real = [c for c in workloads if c['kind'] == 'document']
        self.assertEqual(len(real), 12)
        self.assertEqual(len(workloads), 24)
        for workload, origin in zip(real, original):
            self.assertEqual(workload['source'], origin['source'])
            self.assertEqual(workload['attributes'], origin.get('attributes', {}))
            self.assertEqual(workload['resources'], origin.get('resources', {}))
            self.assertEqual(workload['provenance']['origins'], metadata['origins'])
            self.assertEqual(workload['safe_mode'], 'safe' if 'resources' in origin else 'secure')

    def test_synthetic_inputs_are_deterministic_scaled_and_keep_unique_ids(self):
        for family in perf.FAMILIES:
            sources = [perf.synthetic(family, size) for size in perf.SIZES]
            self.assertEqual(sources, [perf.synthetic(family, size) for size in perf.SIZES])
            self.assertLess(len(sources[0]), len(sources[1]))
            self.assertLess(len(sources[1]), len(sources[2]))
        references = perf.synthetic('references', 32)
        self.assertEqual(references.count('[#s'), 32)
        self.assertIn('<<s0>>', references)  # Last forward reference wraps to a valid target.
        self.assertIn('Ж🙂', perf.synthetic('prose', 32))
        with self.assertRaises(ValueError):
            perf.synthetic('unknown', 32)

    def test_driver_rejects_debug_build_partial_measurements_and_input_drift(self):
        original = driver()
        perf.validate_driver(original, case(), original['settings'])
        changes = [lambda r: r.update(debug_assertions=True), lambda r: r.update(schema='other'),
                   lambda r: r.update(extra=1), lambda r: r.update(source_bytes=1),
                   lambda r: r.update(resource_bytes=0), lambda r: r.update(resource_bytes=False),
                   lambda r: r.update(elapsed_ns=list(perf.OPERATIONS)), lambda r: r.update(settings=[]), lambda r: r['settings'].update(iterations=3),
                   lambda r: r['elapsed_ns'].pop('tokens'), lambda r: r['elapsed_ns'].update(json=[20]),
                   lambda r: r['elapsed_ns'].update(json=[20, 0, 60]),
                   lambda r: r['observations'].update(json_bytes=0),
                   lambda r: r['observations']['diagnostics'].update(warning=1),
                   lambda r: r['observations']['diagnostics'].update(error=-1)]
        for mutate in changes:
            changed = copy.deepcopy(original)
            mutate(changed)
            with self.assertRaises(ValueError):
                perf.validate_driver(changed, case(), original['settings'])

    def test_incomplete_inputs_report_diagnostics_instead_of_discarding_them(self):
        workload = case()
        workload['provenance']['family'] = 'incomplete'
        report = driver(workload)
        report['observations']['diagnostics']['warning'] = 1
        perf.validate_driver(report, workload, report['settings'])
        self.assertEqual(report['observations']['diagnostics']['warning'], 1)

    def test_transport_preserves_utf8_crlf_and_resource_names_outside_timers(self):
        workload = case()
        report = driver(workload)
        def execute(command, **kwargs):
            self.assertEqual(Path(command[command.index('--source') + 1]).read_bytes(), workload['source'].encode())
            at = command.index('--resource')
            self.assertEqual(command[at + 1], 'example$code.adoc')
            self.assertEqual(Path(command[at + 2]).read_bytes(), b'Nested.\r\n')
            self.assertIn('a value with spaces', command)
            self.assertEqual(kwargs['timeout'], 12)
            return subprocess.CompletedProcess(command, 0, json.dumps(report), '')
        with patch('performance.invoke', side_effect=execute):
            result = perf.run_case(Path('/driver'), workload, report['settings'], 12)
        self.assertEqual(result['input']['source_sha256'], perf.digest(workload['source'].encode()))
        self.assertEqual(result['measurements']['parse']['median_ns'], 20)
        self.assertEqual(result['measurements']['parse']['elapsed_ns'], [20, 40, 60])

    def test_timeout_and_stderr_cannot_be_reported_as_success(self):
        workload = case()
        settings = driver()['settings']
        with patch('performance.invoke', side_effect=subprocess.TimeoutExpired('driver', 1)):
            with self.assertRaises(subprocess.TimeoutExpired):
                perf.run_case(Path('/driver'), workload, settings, 1)
        output = subprocess.CompletedProcess([], 0, json.dumps(driver()), 'failure')
        with patch('performance.invoke', return_value=output), self.assertRaises(ValueError):
            perf.run_case(Path('/driver'), workload, settings, 1)

    def test_cargo_artifact_selection_requires_one_existing_output(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'module.wasm'
            path.write_bytes(b'\0asm\x01\0\0\0')
            record = {'reason': 'compiler-artifact', 'target': {'name': 'adoctra_wasm'}, 'filenames': [str(path)], 'executable': None}
            output = json.dumps(record)
            self.assertEqual(perf.artifact(output, 'adoctra_wasm', '.wasm'), path)
            for invalid in ['', output + '\n' + output]:
                with self.assertRaises(ValueError):
                    perf.artifact(invalid, 'adoctra_wasm', '.wasm')
            path.unlink()
            with self.assertRaises(ValueError):
                perf.artifact(output, 'adoctra_wasm', '.wasm')

    def test_wasm_size_identifies_the_raw_artifact_and_deterministic_compression(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'module.wasm'
            data = b'\0asm\x01\0\0\0'
            path.write_bytes(data)
            report = perf.wasm_size(path, ['cargo', 'build'])
            self.assertEqual(report['bytes'], len(data))
            self.assertEqual(report['gzip_bytes'], len(gzip.compress(data, compresslevel=9, mtime=0)))
            self.assertEqual(report['sha256'], perf.digest(data))
            self.assertEqual(report['stage'], 'cargo-cdylib-before-wasm-bindgen')
            path.write_bytes(b'not a module')
            with self.assertRaises(ValueError):
                perf.wasm_size(path, [])

    def test_failed_report_serialization_preserves_previous_report(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'report.json'
            perf.write_report(path, {'good': 1})
            before = path.read_bytes()
            with self.assertRaises(ValueError):
                perf.write_report(path, {'bad': float('nan')})
            self.assertEqual(path.read_bytes(), before)
            self.assertEqual(list(Path(directory).iterdir()), [path])

    def test_invalid_selection_and_settings_never_build(self):
        with patch('performance.build') as build, contextlib.redirect_stderr(io.StringIO()):
            for args in [['--case', 'absent'], ['--samples', '0'], ['--iterations', '-1'],
                         ['--timeout', 'nan'], ['--smoke', '--case', 'x']]:
                with self.assertRaises(SystemExit) as error:
                    perf.main(args)
                self.assertEqual(error.exception.code, 2)
            build.assert_not_called()

    def test_stored_baseline_samples_and_statistics_are_self_consistent(self):
        report = json.loads((perf.ROOT / 'performance/baseline.json').read_text())
        self.assertEqual(report['schema'], perf.SCHEMA)
        self.assertEqual(len({entry['id'] for entry in report['cases']}), 24)
        self.assertEqual(report['wasm']['status'], 'measured')
        self.assertGreater(report['wasm']['bytes'], report['wasm']['gzip_bytes'])
        for entry in report['cases']:
            self.assertEqual(set(entry['measurements']), set(perf.OPERATIONS))
            for measurement in entry['measurements'].values():
                self.assertEqual(len(measurement['elapsed_ns']), report['settings']['samples'])
                expected = perf.summary(measurement['elapsed_ns'], report['settings']['iterations'])
                self.assertEqual({key: measurement[key] for key in expected}, expected)

    def test_source_changes_during_measurement_prevent_publication(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'driver'
            binary.write_bytes(b'driver')
            output = Path(directory) / 'report.json'
            with patch('performance.workloads', return_value=[case()]), \
                 patch('performance.fingerprint', side_effect=['before', 'after']), \
                 patch('performance.build', return_value=(binary, ['cargo'])), \
                 patch('performance.environment', return_value={}), \
                 patch('performance.invoke', return_value=subprocess.CompletedProcess([], 0, 'revision', '')), \
                 patch('performance.run_case', return_value={'measurements': {op: {'median_ns': 1} for op in perf.OPERATIONS}}), \
                 contextlib.redirect_stderr(io.StringIO()), self.assertRaisesRegex(ValueError, 'source changed'):
                perf.main(['--skip-wasm', '--output', str(output)])
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
