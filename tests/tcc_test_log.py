"""Append command receipts without reusing names after suite counter resets."""
# SPDX-License-Identifier: GPL-3.0-or-later
import json
import os
from pathlib import Path
import tempfile


def write_command_log(directory, suite, sequence, args, returncode, output):
    """Create one exclusive JSON receipt; the readable sequence is not its identity."""
    # Keep caller labels inside the selected directory, including on Windows.
    if not suite or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-' for c in suite):
        raise ValueError('invalid command log suite label')
    if type(sequence) is not int or sequence < 0:
        raise ValueError('invalid command log sequence')
    payload = json.dumps({'suite': suite, 'sequence': sequence,
                          'args': [str(arg) for arg in args],
                          'returncode': returncode, 'output': output}, indent=2) + '\n'
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=f'{suite}-{sequence:03d}-', suffix='.json', dir=directory)
    with os.fdopen(fd, 'w', encoding='utf-8') as stream:
        stream.write(payload)
    return Path(name)
