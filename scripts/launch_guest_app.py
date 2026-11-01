#!/usr/bin/env python3
"""Launch a command in Portal Debug's live desktop without changing engines."""
import argparse
import shlex
import sys
from guest_exec import Session


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial')
    parser.add_argument('--log', default='/tmp/portal-app-launch.log')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('supply an application command')
    script = 'exec ' + ' '.join(command)
    return Session.connect(args.serial).run_desktop(
        'nohup /bin/sh -c ' + shlex.quote(script) + ' >' + shlex.quote(args.log)
        + ' 2>&1 </dev/null &', timeout=None).returncode


if __name__ == '__main__':
    sys.exit(main())
