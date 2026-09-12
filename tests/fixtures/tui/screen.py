"""Bounded visible-screen interpreter for the renderer's VT cursor/erase/SGR subset."""
import codecs
import unicodedata

class Screen:
    def __init__(self, rows=24, columns=100):
        self.decoder = codecs.getincrementaldecoder('utf-8')('replace')
        self.escape = ''
        self.style = ()
        self.row = self.column = 0
        self.resize(rows, columns)

    def resize(self, rows, columns):
        self.rows, self.columns = rows, columns
        self.cells = [[' '] * columns for _ in range(rows)]
        self.styles = [[()] * columns for _ in range(rows)]
        self.row = min(self.row, rows-1); self.column = min(self.column, columns-1)

    def feed(self, data):
        for char in self.decoder.decode(data):
            if self.escape:
                self.escape += char
                if len(self.escape) == 2 and char != '[': self.escape = ''
                elif len(self.escape) > 2 and '@' <= char <= '~':
                    args, command = self.escape[2:-1], char
                    if not args.startswith('?'):
                        values = [int(v) if v else 0 for v in args.split(';')]
                        if command in ('H','f'):
                            self.row = min(max((values[0] or 1)-1,0), self.rows-1)
                            self.column = min(max((values[1] if len(values)>1 else 1)-1,0), self.columns-1)
                        elif command == 'J' and values[0] == 2:
                            self.cells = [[' '] * self.columns for _ in range(self.rows)]
                            self.styles = [[()] * self.columns for _ in range(self.rows)]
                        elif command == 'K':
                            for col in range(self.column, self.columns): self.cells[self.row][col] = ' '; self.styles[self.row][col] = ()
                        elif command == 'm':
                            if 0 in values: self.style = ()
                            self.style += tuple(v for v in values if v)
                    self.escape = ''
                elif len(self.escape)>64: raise AssertionError('Unbounded terminal escape')
                continue
            if char == '\x1b': self.escape = char
            elif char == '\r': self.column = 0
            elif char == '\n': self.row = min(self.row+1,self.rows-1)
            elif not char.isprintable(): continue
            elif self.column < self.columns:
                if unicodedata.combining(char): continue
                self.cells[self.row][self.column] = char
                self.styles[self.row][self.column] = self.style
                self.column += 2 if unicodedata.east_asian_width(char) in ('W','F') else 1

    def text(self):
        return '\n'.join(''.join(row).rstrip() for row in self.cells).encode()
