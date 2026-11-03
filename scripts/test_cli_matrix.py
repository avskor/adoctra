import subprocess
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import cli_matrix


class FilesViewTests(unittest.TestCase):
    def test_converted_output_without_an_html_name_is_projected(self):
        # `-o dir/` or `outfilesuffix=.xml`: markup differs by design, so it
        # must compare as a projection, not byte for byte.
        ruby = b'<div class="paragraph">\n<p>text</p>\n</div>'
        ours = b'<p>text</p>'
        self.assertEqual(cli_matrix.files_view({'out/a.xml': ruby}),
                         cli_matrix.files_view({'out/a.xml': ours}))
        self.assertEqual(cli_matrix.files_view({'newdir': ruby}),
                         cli_matrix.files_view({'newdir': ours}))

    def test_other_files_still_compare_bytes(self):
        self.assertEqual(cli_matrix.files_view({'css/my.css': b'body{}\n'}), {'css/my.css': '626f64797b7d0a'})
        self.assertNotEqual(cli_matrix.files_view({'pic.png': b'\x89PNG<'}),
                            cli_matrix.files_view({'pic.png': b'\x89PNG>'}))
        self.assertFalse(cli_matrix.html_content(b'\xff<p>'))


class RunTests(unittest.TestCase):
    def test_binary_files_and_stdin_bytes_reach_the_process(self):
        case = {'id': 'x', 'args': ['in.adoc'], 'files': {'a.adoc': 'a'},
                'binary_files': {'in.adoc': 'fffe2a00'}, 'stdin_hex': 'fffe'}
        done = subprocess.CompletedProcess([], 0, b'', b'')
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(cli_matrix.subprocess, 'run', return_value=done) as run:
            root = Path(directory)
            (root / 'in.adoc').write_bytes(b'\xff\xfe*\x00')
            side = cli_matrix.run(['adoctra'], case, root)
        self.assertEqual(run.call_args.kwargs['input'], b'\xff\xfe')
        self.assertNotIn('in.adoc', side['files'])


if __name__ == '__main__':
    unittest.main()
