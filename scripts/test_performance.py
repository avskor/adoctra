import contextlib
import copy
import gzip
import io
import json
from pathlib import Path
import re
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

    def test_substitution_study_keeps_inputs_and_repeated_raw_timings(self):
        report = json.loads((perf.ROOT / 'performance/substitution-buffers.json').read_text())
        self.assert_study_timings(report, 'adoctra-substitution-buffer-study-v1')

    def assert_study_timings(self, report, schema):
        self.assertEqual(report['schema'], schema)
        originals = {case['id']: case for case in perf.workloads()}
        workloads = {case['id']: case for case in report['workloads']}
        self.assertEqual(set(workloads), set(originals))
        for ident, workload in workloads.items():
            self.assertEqual(workload['input']['source_sha256'], perf.digest(originals[ident]['source'].encode()))
            self.assertEqual(workload['provenance'], originals[ident]['provenance'])
            self.assertEqual(workload['input']['attributes'], originals[ident]['attributes'])
            self.assertEqual(workload['input']['resources'], {
                name: {'bytes': len(value.encode()), 'sha256': perf.digest(value.encode())}
                for name, value in originals[ident]['resources'].items()})
        for series, cases in report['summary'].items():
            runs = [r for r in report['runs'] if r['series'] == series]
            self.assertEqual(len(runs), 6)
            for variant in ['before', 'after']:
                selected = [r for r in runs if r['variant'] == variant]
                self.assertEqual([r['index'] for r in selected], [1, 2, 3])
                for run in selected:
                    self.assertEqual(run['environment']['affinity'], [2])
                    self.assertEqual(set(run['elapsed_ns']), set(cases))
                for ident, operations in cases.items():
                    self.assertEqual(set(operations), set(perf.OPERATIONS))
                    for operation, medians in operations.items():
                        expected = []
                        for run in selected:
                            samples = run['elapsed_ns'][ident][operation]
                            self.assertEqual(len(samples), run['settings']['samples'])
                            expected.append(perf.summary(samples, run['settings']['iterations'])['median_ns'])
                        self.assertEqual(medians[variant + '_run_medians_ns'], expected)

    def test_substitution_study_counters_and_profile_limitations_match_evidence(self):
        report = json.loads((perf.ROOT / 'performance/substitution-buffers.json').read_text())
        self.assert_study_allocations(report)

    def assert_study_allocations(self, report):
        rows = report['allocations']['runs']
        self.assertEqual({(r['variant'], r['id'], r['operation']) for r in rows}, {
            (variant, f'stress-{family}-512', operation)
            for variant in ['before', 'after'] for family in ['references', 'tables']
            for operation in ['parse', 'tokens']})
        for row in rows:
            raw = row['raw_summary']
            values = tuple(map(int, re.search(
                r'heap total: (\d+), heap peak: (\d+), stack peak: (\d+)', raw).groups()))
            self.assertEqual(values, tuple(row[key] for key in ['heap_total_bytes', 'heap_peak_bytes', 'stack_peak_bytes']))
            for name, count in row['calls'].items():
                found = re.search(rf'{name}\|\s+(\d+)\s+(\d+)\s+(\d+)', raw)
                self.assertEqual(count, int(found[1]))
                self.assertEqual(int(found[3]), 0)
            self.assertEqual(row['allocation_calls'], sum(row['calls'].values()))
        for profile in report['profiles']:
            data = (perf.ROOT / profile['path']).read_bytes()
            self.assertEqual(perf.digest(data), profile['sha256'])
            self.assertIn(profile.get('diagnostic', 'Collection interval timer period was changed').encode(), data)
            self.assertFalse(profile['quantitative_cpu_valid'])

    def test_link_stage_study_preserves_inputs_samples_and_profiler_failures(self):
        report = json.loads((perf.ROOT / 'performance/link-stages.json').read_text())
        self.assert_study_timings(report, 'adoctra-link-stage-study-v1')
        self.assert_study_allocations(report)
        self.assertEqual(len(report['runs']), 6)
        self.assertEqual([run['variant'] for run in report['runs']],
                         ['before', 'after', 'after', 'before', 'before', 'after'])

    def test_attribute_snapshot_study_preserves_inputs_samples_and_allocation_evidence(self):
        report = json.loads((perf.ROOT / 'performance/attribute-snapshots.json').read_text())
        self.assert_paired_native_study(report, 'adoctra-attribute-snapshot-study-v1')

    def test_unchanged_stage_study_preserves_inputs_samples_and_allocation_evidence(self):
        report = json.loads((perf.ROOT / 'performance/unchanged-stages.json').read_text())
        self.assert_paired_native_study(report, 'adoctra-unchanged-stage-study-v1')

    def test_owned_part_study_preserves_inputs_samples_and_allocation_evidence(self):
        report = json.loads((perf.ROOT / 'performance/owned-parts.json').read_text())
        self.assert_paired_native_study(report, 'adoctra-owned-part-study-v1', ('paired', 'focused'))
        self.assertEqual(set(report['summary']['focused']),
                         {'stress-incomplete-32', 'stress-incomplete-128', 'stress-tables-32'})
        for run in report['runs']:
            if run['series'] == 'focused':
                self.assertEqual(run['settings'], {'warmup': 4, 'samples': 9, 'iterations': 10})
        process = report['process_times']
        self.assertEqual(process['id'], 'stress-incomplete-128')
        self.assertEqual(process['operation'], 'parse')
        workload = next(case for case in report['workloads'] if case['id'] == process['id'])
        self.assertEqual(process['source_sha256'], workload['input']['source_sha256'])
        self.assertEqual(process['settings'], {'warmup': 20, 'samples': 1, 'iterations': 2000})
        self.assertEqual([run['variant'] for run in process['runs']],
                         ['before', 'after', 'after', 'before', 'before', 'after'])
        for run in process['runs']:
            self.assertEqual(run['environment']['affinity'], [2])
            self.assertEqual(run['native_sha256'],
                             report['allocations']['binaries'][run['variant']]['native_sha256'])
            self.assertGreater(run['wall_ns'], 0)
            self.assertGreater(run['user_cpu_seconds'], 0)
            self.assertGreaterEqual(run['system_cpu_seconds'], 0)
            self.assertEqual(run['driver']['operation'], process['operation'])
            for name, value in process['settings'].items():
                self.assertEqual(run['driver'][name], value)
            self.assertEqual(len(run['driver']['elapsed_ns']), 1)
            self.assertGreater(run['driver']['elapsed_ns'][0], 0)

    def assert_paired_native_study(self, report, schema, series=('paired',)):
        self.assert_study_timings(report, schema)
        self.assert_study_allocations(report)
        self.assertEqual(set(report['summary']), set(series))
        self.assertEqual(len(report['runs']), 6 * len(series))
        for name in series:
            self.assertEqual([run['variant'] for run in report['runs'] if run['series'] == name],
                             ['before', 'after', 'after', 'before', 'before', 'after'])
        self.assertEqual(report['profiles'], [])
        workloads = {case['id']: case for case in report['workloads']}
        for row in report['allocations']['runs']:
            driver = row['driver']
            self.assertEqual(driver['schema'], 'adoctra-profile-driver-v1')
            self.assertEqual(row['source_sha256'], workloads[row['id']]['input']['source_sha256'])
            self.assertEqual(driver['operation'], row['operation'])
            self.assertEqual({key: driver[key] for key in ('warmup', 'samples', 'iterations')},
                             {'warmup': 0, 'samples': 1, 'iterations': 1})
            self.assertEqual(len(driver['elapsed_ns']), 1)
            self.assertGreater(driver['elapsed_ns'][0], 0)
            identity = report['allocations']['binaries'][row['variant']]
            for run in report['runs']:
                if run['variant'] == row['variant']:
                    self.assertEqual(run['code_sha256'], identity['code_sha256'])
                    self.assertEqual(run['native']['sha256'], identity['native_sha256'])


if __name__ == '__main__':
    unittest.main()
