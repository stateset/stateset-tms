"""Evaluate predeclared fixed-arrival budgets, including every offered arrival."""

import math

KINDS = ('read', 'probe', 'health', 'ready', 'write')


def validate_budget(value):
    if not isinstance(value, dict) or set(value) != {'schema', 'kinds', 'max_p99_start_lateness_ms'}:
        raise ValueError('Incomplete capacity budget')
    if type(value['schema']) is not int or value['schema'] != 1 or set(value['kinds']) != set(KINDS):
        raise ValueError('Invalid capacity budget schema/kinds')
    def number(n):
        return type(n) in (int, float) and math.isfinite(n)
    if not number(value['max_p99_start_lateness_ms']) or value['max_p99_start_lateness_ms'] <= 0:
        raise ValueError('Invalid scheduling budget')
    for item in value['kinds'].values():
        if (set(item) != {'min_success_fraction', 'max_p95_ms'}
                or not number(item['min_success_fraction']) or not 0 < item['min_success_fraction'] <= 1
                or not number(item['max_p95_ms']) or item['max_p95_ms'] <= 0):
            raise ValueError('Invalid response budget')
    return value


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)] if ordered else None


def evaluate_budget(report, budget, schedule):
    validate_budget(budget)
    violations, observations = [], {}
    arrivals, responses = {}, {}
    for item in report['arrivals']:
        if item['name'] in arrivals:
            violations.append('duplicate_arrival:' + item['name'])
        arrivals[item['name']] = item
    for item in report['requests']:
        if not item['name'].startswith('arrival-') or item['name'].endswith('-replay'):
            continue
        if item['name'] in responses:
            violations.append('duplicate_response:' + item['name'])
        responses[item['name']] = item
    expected_names = set()
    for kind in KINDS:
        expected = {'arrival-%s-%04d' % (kind, n) for n, _ in enumerate(schedule(
            report['declared_sustained']['duration_seconds'],
            report['declared_sustained'][kind + '_interval_ms']))}
        expected_names.update(expected)
        successes, latencies, lateness = 0, [], []
        for name in sorted(expected):
            arrival, response = arrivals.get(name), responses.get(name)
            if not arrival or not response or 'not_sent' in arrival:
                violations.append('missing_or_unsent:' + name)
                continue
            if response.get('status') == 200 and not any(k in response for k in ('error', 'harness_error', 'not_sent')):
                successes += 1
            else:
                # Explicit overload responses still count against availability.
                if response.get('status') not in (200, 503) or any(k in response for k in ('error', 'harness_error')):
                    violations.append('invalid_response:' + name)
            start_lateness = arrival.get('start_lateness_ms')
            wall = response.get('wall_ms')
            if (type(start_lateness) not in (int, float) or not math.isfinite(start_lateness)
                    or start_lateness < 0 or type(wall) not in (int, float)
                    or not math.isfinite(wall) or wall < 0):
                violations.append('missing_or_invalid_timing:' + name)
                continue
            lateness.append(start_lateness)
            latencies.append(start_lateness + wall)
        fraction = successes / len(expected)
        p95, p99_lateness = percentile(latencies, .95), percentile(lateness, .99)
        observations[kind] = {'offered': len(expected), 'successes': successes,
                              'success_fraction': fraction, 'p95_offered_to_response_ms': p95,
                              'p99_start_lateness_ms': p99_lateness}
        if fraction < budget['kinds'][kind]['min_success_fraction']:
            violations.append('availability:' + kind)
        if p95 is None or p95 > budget['kinds'][kind]['max_p95_ms']:
            violations.append('latency:' + kind)
        if p99_lateness is None or p99_lateness > budget['max_p99_start_lateness_ms']:
            violations.append('client_scheduling:' + kind)
    if set(arrivals) != expected_names or set(responses) != expected_names:
        violations.append('offered_response_inventory_mismatch')
    return {'result': 'pass' if not violations else 'fail', 'kinds': observations,
            'violations': violations,
            'scope': 'Finite declared fixed-arrival local workload; p95 includes client dispatch lateness and all completed statuses; unsent/missing/invalid responses fail independently.'}
