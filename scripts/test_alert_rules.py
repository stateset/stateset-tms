"""Keep the Prometheus alert rules bound to exported metrics and documented runbooks."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
RULES = ROOT/'monitoring/freight-alerts.yml'
DOCS = ROOT/'docs/observability.md'
SOURCES = [ROOT/'src/observability.rs', ROOT/'src/admission_diagnostics.rs']
EXTERNAL = {'up', 'probe_success'}
SEVERITIES = {'page', 'ticket'}


def alerts():
    text = RULES.read_text()
    blocks = re.split(r'^      - alert: ', text, flags=re.M)[1:]
    parsed = []
    for block in blocks:
        name, _, body = block.partition('\n')
        expr = re.search(r'^        expr: \|\n((?:          .*\n)+)', body, re.M)
        fields = dict(re.findall(r'^ {8,10}(\w+): (.+)$', body, re.M))
        parsed.append({**fields, 'name': name.strip(), 'expr': expr and expr.group(1)})
    return parsed


def exported():
    names = set()
    for source in SOURCES:
        names |= set(re.findall(r'\bfreight_[a-z_]+\b', source.read_text()))
    return names


class AlertRules(unittest.TestCase):
    def test_every_alert_is_complete_and_unique(self):
        rules = alerts()
        self.assertGreater(len(rules), 0)
        names = [rule['name'] for rule in rules]
        self.assertEqual(len(names), len(set(names)))
        for rule in rules:
            with self.subTest(alert=rule['name']):
                self.assertRegex(rule['name'], r'^Freight[A-Z][A-Za-z]+$')
                self.assertRegex(rule['expr'] or '', r'[a-z_]+', 'expr must be a literal block')
                self.assertIn(rule.get('severity'), SEVERITIES)
                for field in ('summary', 'description', 'runbook'):
                    self.assertTrue(rule.get(field), field)
                if 'for' in rule:
                    self.assertRegex(rule['for'], r'^\d+[mh]$')

    def test_expressions_use_only_exported_series(self):
        known = exported()
        for rule in alerts():
            with self.subTest(alert=rule['name']):
                selectors = set(re.findall(r'\b([a-z_][a-z0-9_]*)\{', rule['expr']))
                metrics = set(re.findall(r'\bfreight_[a-z_]+\b', rule['expr']))
                self.assertLessEqual(metrics, known)
                self.assertLessEqual(selectors - metrics, EXTERNAL)

    def test_runbooks_point_at_documented_headings(self):
        anchors = {
            re.sub(r'[^a-z0-9 -]', '', heading.strip().lower()).replace(' ', '-')
            for heading in re.findall(r'^#+ (.+)$', DOCS.read_text(), re.M)
        }
        for rule in alerts():
            with self.subTest(alert=rule['name']):
                document, _, anchor = rule['runbook'].partition('#')
                self.assertEqual(document, 'docs/observability.md')
                self.assertEqual(anchor, rule['name'].lower())
                self.assertIn(anchor, anchors)


if __name__ == '__main__':
    unittest.main()
