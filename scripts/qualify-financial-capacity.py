#!/usr/bin/env python3
"""Qualify declared financial arrivals and budgets on source-bound image limits.

Retains the complete inherited originals/receipt/replay and resource controls.
The aggregate read-only observer is compared with the legacy SQL before and
after traffic. This establishes only the exact finite local workload recorded.
"""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import time

from capacity_budget import evaluate_budget, validate_budget
from financial_observer import substitute, validation_sql

spec = importlib.util.spec_from_file_location(
    'financial_capacity_base', Path(__file__).with_name('qualify-api-sustained-envelope.py'))
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)


class FinancialCapacity(base.SustainedEnvelope):
    def __init__(self, args):
        super().__init__(args)
        self.budget = validate_budget(self.plan['capacity_budget'])
        self.report.update(scope=__doc__, capacity_budget=self.budget,
                           observer_equivalence=[], client_phase_observations=[])
        for name in ['qualify-financial-capacity.py', 'financial_observer.py', 'capacity_budget.py']:
            self.report['harness']['scripts/' + name] = base.base.sha(Path(__file__).with_name(name))

    def sql(self, database, statement):
        return super().sql(database, substitute(statement))

    def observe_write(self, name):
        started = time.monotonic()
        try:
            return super().observe_write(name)
        finally:
            self.report['client_phase_observations'].append({
                'phase': 'guarded_write_and_exact_original_verification', 'name': name,
                'wall_ms': (time.monotonic() - started) * 1000})

    def legacy_equivalence(self, phase):
        before = self.physical()
        for tenant in ['large', 'probe', 'writer']:
            statement = 'SELECT freight_workspace_originals(' + base.base.literal(tenant) + ')::text'
            legacy = super().sql('freight_runtime', statement)
            current = self.sql('freight_runtime', statement)
            base.require(legacy == current, 'Observer changed exact original SQL bytes')
            self.report['observer_equivalence'].append({
                'phase': phase, 'tenant': tenant, 'exact_text_equal': True,
                'bytes': len(legacy.encode()), 'sha256': hashlib.sha256(legacy.encode()).hexdigest()})
        base.require(self.physical() == before, 'Observer rewrote canonical rows')

    def checks(self):
        statement, expected = validation_sql()
        before = self.physical()
        # psql -c/PQexec exposes only the final result of a multi-command
        # string. Read stdin so every case result survives the final rollback.
        output = self.command([
            'psql', 'postgres://freight_runtime@127.0.0.1:%d/freight_runtime' % self.ports['postgres'],
            '-X', '-At', '-v', 'ON_ERROR_STOP=1'], input=statement,
            env={'PATH': os.defpath, 'LC_ALL': 'C'}, label='transaction-local observer controls')
        cases = [json.loads(line) for line in output.splitlines() if line.startswith('{')]
        self.report['observer_validation'] = cases
        base.require(len(cases) == len(expected) and {c['case'] for c in cases} == set(expected)
                     and all(c['exact_text_equal'] and c['legacy_valid'] == expected[c['case']] for c in cases),
                     'Actual observer corruption/precision controls failed')
        base.require(self.physical() == before, 'Observer controls changed serving originals')
        self.legacy_equivalence('before_offered_traffic')
        try:
            super().checks()
            self.legacy_equivalence('after_offered_traffic_and_replays')
        finally:
            if 'declared_sustained' in self.report:
                self.report['capacity_budget_evaluation'] = evaluate_budget(self.report, self.budget, base.schedule)
        base.require(self.report['capacity_budget_evaluation']['result'] == 'pass',
                     'Declared finite capacity budgets failed')


if __name__ == '__main__':
    raise SystemExit(base.base.main(FinancialCapacity))
