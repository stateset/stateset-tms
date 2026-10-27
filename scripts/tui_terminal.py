"""Small incremental screen reader for owned TUI acceptance fixtures.

Tracks the cursor, erasure and UTF-8 used by ratatui. This is a test observer,
not a terminal emulator or a parser for untrusted production input.
"""
import codecs
import unicodedata


class Screen:
    def __init__(self, rows, columns):
        self.rows, self.columns = rows, columns
        self.cells = [[' '] * columns for _ in range(rows)]
        self.row = self.column = 0
        self.saved = (0, 0)
        self.pending = ''
        self.decoder = codecs.getincrementaldecoder('utf-8')(errors='replace')

    @property
    def text(self):
        return '\n'.join(''.join(row).rstrip() for row in self.cells)

    def feed(self, data):
        text = self.pending + self.decoder.decode(data)
        self.pending = ''
        i = 0
        while i < len(text):
            ch = text[i]
            if ch == '\x1b':
                start = i
                if i + 1 == len(text):
                    self.pending = text[i:]
                    break
                kind = text[i + 1]
                if kind == '[':
                    i += 2
                    while i < len(text) and not ('@' <= text[i] <= '~'):
                        i += 1
                    if i == len(text):
                        self.pending = text[start:]
                        break
                    self._csi(text[start + 2:i], text[i])
                elif kind == ']':
                    i += 2
                    while i < len(text) and text[i] != '\a' and text[i:i+2] != '\x1b\\':
                        i += 1
                    if i == len(text):
                        self.pending = text[start:]
                        break
                    if text[i] == '\x1b':
                        i += 1
                else:
                    i += 1
                    if kind == '7':
                        self.saved = self.row, self.column
                    elif kind == '8':
                        self.row, self.column = self.saved
                i += 1
                continue
            if ch == '\r':
                self.column = 0
            elif ch == '\n':
                self.row = min(self.row + 1, self.rows - 1)
            elif ch == '\b':
                self.column = max(0, self.column - 1)
            elif ch >= ' ' and ch != '\x7f':
                if unicodedata.combining(ch):
                    if self.column:
                        self.cells[self.row][min(self.column, self.columns) - 1] += ch
                else:
                    width = 2 if unicodedata.east_asian_width(ch) in ('W', 'F') else 1
                    if self.column < self.columns:
                        self.cells[self.row][self.column] = ch
                        if width == 2 and self.column + 1 < self.columns:
                            self.cells[self.row][self.column + 1] = ''
                    self.column = min(self.columns, self.column + width)
            i += 1

    def _csi(self, raw, final):
        # Private modes, colors and device status queries do not paint cells.
        if any(c not in '0123456789;' for c in raw):
            return
        values = [int(v) if v else 0 for v in raw.split(';')]
        amount = values[0] or 1
        if final in ('H', 'f'):
            self.row = min(self.rows - 1, max(0, amount - 1))
            self.column = min(self.columns - 1, max(0, (values[1] if len(values) > 1 else 1) - 1))
        elif final in ('A', 'B', 'C', 'D'):
            if final == 'A': self.row = max(0, self.row - amount)
            if final == 'B': self.row = min(self.rows - 1, self.row + amount)
            if final == 'C': self.column = min(self.columns - 1, self.column + amount)
            if final == 'D': self.column = max(0, self.column - amount)
        elif final == 'G':
            self.column = min(self.columns - 1, amount - 1)
        elif final == 'd':
            self.row = min(self.rows - 1, amount - 1)
        elif final == 'J':
            for row in range(self.rows):
                for column in range(self.columns):
                    at = (row, column)
                    if values[0] in (2, 3) or values[0] == 0 and at >= (self.row, self.column) or values[0] == 1 and at <= (self.row, self.column):
                        self.cells[row][column] = ' '
        elif final == 'K':
            for column in range(self.columns):
                if values[0] == 2 or values[0] == 0 and column >= self.column or values[0] == 1 and column <= self.column:
                    self.cells[self.row][column] = ' '
        elif final == 's':
            self.saved = self.row, self.column
        elif final == 'u':
            self.row, self.column = self.saved
