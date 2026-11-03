"""diagnostic-positions-v1: ordered severity, text and primary file/line.

Absent Ruby logger cursors and unlocated backend warnings stay null. Columns,
byte ranges and secondary include_location cursors are outside this profile.
"""

import re

PROFILE = 'diagnostic-positions-v1'


def source_name(name, root=None):
    if root is None and name in {'-', '<stdin>'}:
        return '<stdin>'
    if root is not None:
        prefix = str(root) + '/'
        if name.startswith(prefix):
            relative = name[len(prefix):]
            if all(part not in {'', '.', '..'} for part in relative.split('/')):
                return '<fixture>/' + relative
    raise ValueError(f'unexpected diagnostic source: {name!r}')


def validate(report):
    if not isinstance(report, dict) or set(report) != {'diagnostics'} or not isinstance(report['diagnostics'], list):
        raise ValueError('invalid diagnostic positions report')
    for diagnostic in report['diagnostics']:
        if (not isinstance(diagnostic, dict) or set(diagnostic) != {'severity', 'message', 'location'}
                or diagnostic['severity'] not in {'warning', 'error', 'fatal', 'unknown'}
                or not isinstance(diagnostic['message'], str)):
            raise ValueError('invalid positioned diagnostic')
        location = diagnostic['location']
        if location is None:
            continue
        if (not isinstance(location, dict) or set(location) != {'file', 'line'}
                # Line 0: Ruby's `cursor_at_prev_line` on a first line.
                or type(location['line']) is not int or location['line'] < 0
                or not isinstance(location['file'], str)):
            raise ValueError('invalid diagnostic location')
        name = location['file']
        if name != '<stdin>':
            if not name.startswith('<fixture>/'):
                raise ValueError('noncanonical diagnostic source')
            parts = name[len('<fixture>/'):].split('/')
            if any(part in {'', '.', '..'} for part in parts) or any(c in name for c in '\\:\0'):
                raise ValueError('invalid diagnostic source path')
    return report


def reference_projection(report, root=None):
    # Validate after canonicalizing only the two transport-dependent names.
    # Missing fields, invalid locations and unknown schema must still fail.
    if isinstance(report, dict) and isinstance(report.get('diagnostics'), list):
        for diagnostic in report['diagnostics']:
            if not isinstance(diagnostic, dict):
                continue
            location = diagnostic.get('location')
            if isinstance(location, dict) and isinstance(location.get('file'), str):
                location['file'] = source_name(location['file'], root)
            if root is not None and isinstance(diagnostic.get('message'), str):
                diagnostic['message'] = diagnostic['message'].replace(str(root), '<fixture>')
    return validate(report)


def cli_projection(stderr, root=None):
    diagnostics = []
    for line in stderr.splitlines():
        match = re.fullmatch(
            r'adoctra: (.+?):([1-9]\d*):([1-9]\d*): (warning|error|info) at (\d+)\.\.(\d+): (.*)', line)
        if match:
            name, lineno, _column, severity, start, end, message = match.groups()
            if int(end) < int(start):
                raise ValueError('reversed diagnostic byte range')
            location = {'file': source_name(name, root), 'line': int(lineno)}
        else:
            match = re.fullmatch(r'adoctra: (.+?):(0|[1-9]\d*): (warning|error|info): (.*)', line)
            if match:
                name, lineno, severity, message = match.groups()
                location = {'file': source_name(name, root), 'line': int(lineno)}
            else:
                match = re.fullmatch(r'adoctra: (warning|error|info): (.*)', line)
                if not match:
                    raise ValueError(f'unrecognized adoctra diagnostic: {line!r}')
                severity, message = match.groups()
                location = None
        if severity == 'info':
            continue
        if root is not None:
            message = message.replace(str(root), '<fixture>')
        diagnostics.append({'severity': severity, 'message': message, 'location': location})
    return validate({'diagnostics': diagnostics})
