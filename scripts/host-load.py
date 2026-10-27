#!/usr/bin/env python3
"""Keep shared-host CPU starvation from reading as a product verdict in the release gate.

`preflight` refuses to start on a host that is already overloaded. `monitor` records the
peak load while the gate runs. `classify` passes a successful status through unchanged and
labels a failure that coincided with overload. A labelled failure still fails; it never
becomes a pass.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import sys
import time

OVERLOADED = 75  # EX_TEMPFAIL: retry on a quiet host.
LOADAVG = Path('/proc/loadavg')
DEFAULT_START = 1.0
DEFAULT_FAILURE = 2.0


def cpus():
    try:
        return len(os.sched_getaffinity(0))
    except AttributeError:
        return os.cpu_count() or 1


def load(path=LOADAVG):
    one, five, *_ = path.read_text().split()
    return float(one), float(five)


def limit(value, variable, default):
    raw = os.environ.get(variable, '') if value is None else value
    try:
        number = float(raw) if raw != '' else default
    except ValueError:
        raise SystemExit(f'{variable} must be a positive number of runnable tasks per CPU.')
    if not number > 0:
        raise SystemExit(f'{variable} must be a positive number of runnable tasks per CPU.')
    return number


def preflight(args):
    per_cpu = limit(args.max_load_per_cpu, 'FREIGHT_RELEASE_START_LOAD_PER_CPU', DEFAULT_START)
    _, five = load(args.loadavg)
    count = args.cpus or cpus()
    if five > per_cpu*count:
        print(f'Release gate not started: host overloaded (5-minute load {five:.1f} on {count} CPUs, '
              f'limit {per_cpu*count:.1f}). Wait for a quiet host or raise '
              'FREIGHT_RELEASE_START_LOAD_PER_CPU.', file=sys.stderr)
        return OVERLOADED
    return 0


def write(record, output):
    partial = output.with_name(output.name + '.partial')
    partial.write_text(json.dumps(record, sort_keys=True))
    partial.replace(output)


def monitor(args):
    record = {'cpus': args.cpus or cpus(), 'samples': 0, 'peak_load1': 0.0, 'peak_at': None}
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(True))
    parent = os.getppid()
    while not stop and os.getppid() == parent:
        one, _ = load(args.loadavg)
        record['samples'] += 1
        if one >= record['peak_load1']:
            record['peak_load1'] = one
            record['peak_at'] = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())
        write(record, args.output)
        time.sleep(args.interval)
    return 0


def classify(args):
    if args.status == 0:
        return 0
    per_cpu = limit(args.max_load_per_cpu, 'FREIGHT_RELEASE_FAILURE_LOAD_PER_CPU', DEFAULT_FAILURE)
    try:
        record = json.loads(args.record.read_text())
        peak, count = float(record['peak_load1']), int(record['cpus'])
    except (OSError, ValueError, KeyError, TypeError):
        print('Release gate failed; host load was not recorded.', file=sys.stderr)
        return args.status
    if peak > per_cpu*count:
        print(f'Release gate failed while the host was overloaded: 1-minute load peaked at {peak:.1f} '
              f'on {count} CPUs at {record.get("peak_at")}, above {per_cpu*count:.1f}. This run is not a '
              'product verdict. Inspect the first failure above, then rerun on a quiet host.',
              file=sys.stderr)
        return OVERLOADED
    print(f'Release gate failed with host load within limits (1-minute peak {peak:.1f} on {count} CPUs).',
          file=sys.stderr)
    return args.status


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    for name in ('preflight', 'monitor', 'classify'):
        command = commands.add_parser(name)
        command.add_argument('--loadavg', type=Path, default=LOADAVG, help=argparse.SUPPRESS)
        command.add_argument('--cpus', type=int, default=None, help=argparse.SUPPRESS)
    commands.choices['preflight'].add_argument('--max-load-per-cpu')
    commands.choices['monitor'].add_argument('--output', type=Path, required=True)
    commands.choices['monitor'].add_argument('--interval', type=float, default=5.0)
    commands.choices['classify'].add_argument('--record', type=Path, required=True)
    commands.choices['classify'].add_argument('--status', type=int, required=True)
    commands.choices['classify'].add_argument('--max-load-per-cpu')
    args = parser.parse_args(argv)
    return {'preflight': preflight, 'monitor': monitor, 'classify': classify}[args.command](args)


if __name__ == '__main__':
    sys.exit(main())
