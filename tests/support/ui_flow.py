import fcntl
import json
import os
import pathlib
import pty
import re
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unicodedata

BINARY = sys.argv[1]
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
SERVER = '''import json, os, socket, sys
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', int(sys.argv[1])))
s.listen(8)
with open(sys.argv[2], 'w') as out:
    json.dump({'pid': os.getpid()}, out)
while True:
    conn, _ = s.accept()
    conn.close()
'''


def width(text):
    return sum(0 if unicodedata.combining(c) else 2 if unicodedata.east_asian_width(c) in 'WF' else 1 for c in text)


class UI:
    def __init__(self, root, columns=110):
        self.master, self.slave = pty.openpty()
        self.before = termios.tcgetattr(self.slave)
        self.resize(columns)
        env = os.environ.copy()
        env.update(TERM='xterm-256color', NO_COLOR='1',
                   PATH=str(root / 'bin') + os.pathsep + env['PATH'], DEVPORTS_OPEN_RECORD=str(root / 'opened'))
        self.child = subprocess.Popen([BINARY], stdin=self.slave, stdout=self.slave, stderr=self.slave, env=env, cwd=root)
        self.raw = b''
        self.frame = ''

    def resize(self, columns):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, columns, 0, 0))

    def pump(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.05)[0]:
                try:
                    self.raw += os.read(self.master, 65536)
                except OSError:
                    return
        latest = self.raw.rsplit(b'\x1b[J', 1)[-1]
        self.frame = ANSI.sub('', latest.decode('utf-8', errors='replace')).replace('\r', '')

    def send(self, data):
        os.write(self.master, data.encode() if isinstance(data, str) else data)

    def wait(self, text, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump(0.1)
            if text in self.frame and self.raw.endswith(b'\x1b[0m'):
                return self.frame
            assert self.child.poll() is None, 'exited early:\n' + self.frame
        raise AssertionError('Waiting for ' + repr(text) + '\nLast frame:\n' + self.frame)

    def row(self, port):
        return next(line for line in self.frame.splitlines() if ':' + str(port) in line)

    def finish(self):
        deadline = time.monotonic() + 5
        while self.child.poll() is None and time.monotonic() < deadline:
            self.pump(0.05)
        assert self.child.poll() == 0, 'did not exit cleanly:\n' + self.frame
        self.pump(0.1)
        assert termios.tcgetattr(self.slave) == self.before, 'terminal mode not restored'
        assert b'\x1b[?2004l\x1b[?25h' in self.raw, 'cursor or paste mode not restored'
        assert b'\x1b[?1049h' not in self.raw, 'used the alternate screen'
        os.close(self.master)
        os.close(self.slave)


with tempfile.TemporaryDirectory(prefix='devports-ui-fixture-') as temporary:
    root = pathlib.Path(temporary).resolve()
    script = root / 'server.py'
    script.write_text(SERVER)
    (root / 'bin').mkdir()
    opener = root / 'bin' / ('open' if sys.platform == 'darwin' else 'xdg-open')
    opener.write_text('#!/bin/sh\nprintf "%s\\n" "$1" > "$DEVPORTS_OPEN_RECORD"\n')
    opener.chmod(0o700)
    children = []

    def start_server():
        reservation = socket.socket()
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
        reservation.close()
        ready = root / ('ready-' + str(port) + '.json')
        child = subprocess.Popen([sys.executable, str(script), str(port), str(ready)], cwd=root,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        children.append(child)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            try:
                json.loads(ready.read_text())
                return port, child
            except (FileNotFoundError, json.JSONDecodeError):
                time.sleep(0.03)
        raise AssertionError('Fixture did not start')

    try:
        old_port, old_child = start_server()
        time.sleep(1.2)  # Start times have one-second resolution.
        new_port, _ = start_server()

        ui = UI(root)
        ui.wait(':' + str(old_port))
        lines = ui.frame.splitlines()
        assert lines[0] == '' and lines[1] == 'devports — A CLI to manage ports held by development servers.', ui.frame
        assert ui.frame.endswith('q Quit\n'), repr(ui.frame[-40:])
        assert ui.row(new_port).startswith('→ '), ui.frame
        assert lines.index(ui.row(new_port)) < lines.index(ui.row(old_port)), ui.frame
        assert 'Enter Open' in ui.frame and 'k Kill' in ui.frame, ui.frame

        ui.send('\r')
        ui.wait('Opened http://')
        assert (root / 'opened').read_text().strip() == 'http://127.0.0.1:' + str(new_port)

        ui.send('\x1b[B')
        ui.wait('→ :' + str(old_port), timeout=0.5)  # Keys redraw at once, not on refresh.
        ui.send('\x1b[A')
        ui.wait('→ :' + str(new_port), timeout=0.5)
        ui.send('\x1b[B')
        ui.wait('→ :' + str(old_port), timeout=0.5)
        ui.send('\x1b[200~q k\x1b[201~')
        ui.pump(0.4)
        assert ui.child.poll() is None, 'pasted text acted as a shortcut'
        assert old_child.poll() is None, 'pasted text killed a server'

        ui.send('k')
        ui.wait('✓ Killed :' + str(old_port))
        killed_frame = ui.frame
        old_child.wait(timeout=5)
        deadline = time.monotonic() + 6
        while 'Killed' in ui.frame:  # Result lines clear themselves.
            assert time.monotonic() < deadline, 'kill message stayed:\n' + ui.frame
            ui.pump(0.2)
        assert ':' + str(old_port) + ' ' not in killed_frame.replace('Killed :' + str(old_port), ''), killed_frame

        ui.resize(40)
        os.kill(ui.child.pid, signal.SIGWINCH)
        ui.pump(1.0)
        assert ':' + str(new_port) in ui.frame, ui.frame
        assert all(width(line) <= 39 for line in ui.frame.splitlines()), ui.frame

        ui.send('q')
        ui.finish()
        assert b'Running' in ui.raw
        # The frame ends with a blank line, then the prompt starts on a fresh line.
        assert re.search(rb'Qu\S*\r\n\x1b\[0m\x1b\[0m\x1b\[\?2004l\x1b\[\?25h\r*\n$', ui.raw), ui.raw[-80:]

        for stop in ('\x03', signal.SIGTERM):
            ui = UI(root)
            ui.wait(':' + str(new_port))
            if isinstance(stop, str):
                ui.send(stop)
            else:
                ui.child.send_signal(stop)
            ui.finish()
        print('PTY flow: list, open, paste, kill, resize, quit, Ctrl-C and SIGTERM passed.')
    finally:
        for child in children:
            if child.poll() is None:
                child.kill()
                child.wait()
