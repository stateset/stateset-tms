#!/usr/bin/env python3
"""Check actual browser CSV downloads with Python's CSV reader and LibreOffice Calc."""
import argparse
import csv
import hashlib
import io
import json
from decimal import Decimal
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET
import zipfile

HEADERS = [
    'Reference', 'Customer', 'Origin', 'Destination', 'Status', 'Carrier',
    'Buy USD', 'Sell USD', 'Margin USD', 'Base buy USD',
    'Net buy additions and adjustments USD', 'Base sell USD',
    'Net sell additions and credits USD',
]
NS = {'t': 'urn:oasis:names:tc:opendocument:xmlns:table:1.0',
      'o': 'urn:oasis:names:tc:opendocument:xmlns:office:1.0'}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def one(root, name):
    paths = list(root.rglob(name))
    require(len(paths) == 1, f'Expected one {name}, found {paths}')
    return paths[0]


def read_csv(path):
    with path.open(encoding='utf-8-sig', newline='') as stream:
        rows = list(csv.reader(stream))
    require(rows[0] == HEADERS, 'CSV column contract changed')
    require(all(len(row) == 13 for row in rows), 'CSV record split or column loss')
    return rows


def calc(path, work, name):
    out = work / name
    out.mkdir()
    result = subprocess.run([
        'libreoffice', '-env:UserInstallation=' + (out / 'profile').as_uri(),
        '--headless',
        '--infilter=Text - txt - csv (StarCalc):44,34,76,1,,1033,false,true,false,false,true',
        '--convert-to', 'ods', '--outdir', str(out), str(path),
    ], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=45)
    (out / 'conversion.log').write_text(result.stdout)
    ods = out / (path.stem + '.ods')
    require(result.returncode == 0 and ods.is_file(), f'Calc import failed: {result.stdout}')
    with zipfile.ZipFile(ods) as archive:
        xml = ET.fromstring(archive.read('content.xml'))
    rows = []
    for row in xml.findall('.//t:table-row', NS):
        cells = []
        for cell in row.findall('t:table-cell', NS):
            value = {
                'type': cell.get('{' + NS['o'] + '}value-type'),
                'value': cell.get('{' + NS['o'] + '}value'),
                'formula': cell.get('{' + NS['t'] + '}formula'),
                'text': ''.join(cell.itertext()),
            }
            repeat = int(cell.get('{' + NS['t'] + '}number-columns-repeated', '1'))
            cells.extend([value] * min(repeat, 13 - len(cells)))
        rows.append(cells)
    (out / 'cells.json').write_text(json.dumps(rows, indent=2, ensure_ascii=False) + '\n')
    return rows, {'path': str(ods), 'sha256': digest(ods)}


def sum_copy(path, work, name):
    # Original downloaded bytes remain unchanged; SUM exists only in this copy.
    extra = ['Qualification SUM'] + [''] * 12
    extra[8] = '=SUM(I2:I3)'
    stream = io.StringIO(newline='')
    csv.writer(stream, lineterminator='\r\n', quoting=csv.QUOTE_ALL).writerow(extra)
    body = path.read_bytes()
    target = work / (name + '.csv')
    target.write_bytes(body + (b'' if body.endswith(b'\n') else b'\r\n') + stream.getvalue().encode())
    cells, artifact = calc(target, work, name)
    require(cells[3][8]['formula'] == 'of:=SUM([.I2:.I3])', 'SUM control was not evaluated')
    return cells, artifact


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--before-artifacts', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    work = Path(tempfile.mkdtemp(prefix='freight-export-qualification-'))
    downloaded = one(args.artifacts, 'load-board.csv')
    formula_csv = one(args.artifacts, 'formula-text.csv')
    manifest = json.loads(one(args.artifacts, 'load-board-source.json').read_text())
    records = read_csv(downloaded)
    source = {load['reference']: load for load in manifest['loads']}
    require(len(records) == 3 and {row[0] for row in records[1:]} == set(source), 'Filtered row selection differs')
    require(manifest['excluded']['reference'] not in source, 'Excluded fixture overlaps selection')
    for row in records[1:]:
        load = source[row[0]]
        require(row[1] == manifest['customer']['name'], 'Customer text changed in CSV')
        # These owned fixtures have only their original base agreements.
        buy = load['buy_cents']
        sell = load['sell_cents']
        cents = [buy, sell, sell - buy, load['buy_cents'], buy - load['buy_cents'],
                 load['sell_cents'], sell - load['sell_cents']]
        require([Decimal(value) * 100 for value in row[6:]] == cents, 'CSV amounts differ from original source cents')
    expected_sum = sum(Decimal(row[8]) for row in records[1:])
    formula_source = json.loads(one(args.artifacts, 'formula-text-source.json').read_text())
    guarded = read_csv(formula_csv)
    require(guarded[1][1] == "'" + formula_source['customer']['name'], 'Formula text guard or original bytes lost')
    cells, after_ods = sum_copy(downloaded, work, 'after-sum')
    for i, row in enumerate(records[1:], 1):
        for j in range(6, 13):
            require(cells[i][j]['type'] == 'float' and not cells[i][j]['formula'], 'Money imported as text/formula')
            require(Decimal(cells[i][j]['value']) == Decimal(row[j]), 'Calc amount changed')
    require(Decimal(cells[3][8]['value']) == expected_sum, 'Calc SUM disagrees with original source cents')
    guarded_cells, guarded_ods = calc(formula_csv, work, 'guarded-text')
    require(guarded_cells[1][1]['type'] == 'string', 'Guarded customer is not text')
    require(not any(cell['formula'] for row in guarded_cells for cell in row), 'Export contains spreadsheet formulas')
    # A benign formula proves this configuration actually evaluates unguarded formulas.
    control = work / 'control.csv'
    control.write_text(
        '\ufeff"Plain formula","Quoted whitespace formula","Guarded formula","Unquoted whitespace formula"\r\n'
        '"=1+1","  =1+1","\'  =1+1",  =1+1\r\n', encoding='utf-8')
    controls, control_ods = calc(control, work, 'control')
    require(all(controls[1][i]['formula'] and controls[1][i]['value'] == '2' for i in (0, 3)), 'Formula controls did not evaluate')
    require(controls[1][2]['type'] == 'string' and not controls[1][2]['formula'], 'Guard control evaluated')
    report = {
        'result': 'pass', 'scope': 'Owned actual browser downloads; Python CSV/Decimal and installed Calc import only',
        'directory': str(work), 'libreoffice_version': subprocess.check_output(['libreoffice', '--version'], text=True).strip(),
        'inputs': {str(path): digest(path) for path in (downloaded, formula_csv)},
        'filtered_records': 2, 'columns': 13, 'numeric_money_cells_checked': 14,
        'expected_source_margin_sum': str(expected_sum), 'calc_margin_sum': cells[3][8]['value'],
        'guarded_customer_type': guarded_cells[1][1]['type'],
        'formula_controls': controls[1],
        'spreadsheets': {'after_sum': after_ods, 'guarded_text': guarded_ods, 'control': control_ods},
        'limits': 'No Excel/save-reopen or arbitrary-magnitude spreadsheet precision qualification; SUM added only to separate copies.',
    }
    if args.before_artifacts:
        before = one(args.before_artifacts, 'load-board.csv')
        previous, previous_ods = sum_copy(before, work, 'before-sum')
        require(previous[1][8]['type'] == 'string', 'Preceding defect was not reproduced')
        require(Decimal(previous[3][8]['value']) == Decimal('1000.02'), 'Unexpected preceding SUM')
        report['before'] = {'input': str(before), 'sha256': digest(before),
                            'loss_cell': previous[1][8], 'calc_margin_sum': previous[3][8]['value'], 'spreadsheet': previous_ods}
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')
    print(json.dumps({'result': 'pass', 'margin_sum': str(expected_sum), 'directory': str(work)}))


if __name__ == '__main__':
    main()
