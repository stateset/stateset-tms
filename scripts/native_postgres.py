"""Owned PostgreSQL 16 for local service qualification; no external DB attachment."""
import hashlib
import os
from pathlib import Path
import signal
import socket
import subprocess


class NativePostgres:
    def __init__(self, directory, bin_dir, port):
        requested = Path(directory)
        if requested.is_symlink():
            raise ValueError('Native PostgreSQL requires an owned directory, not a symlink')
        self.directory = requested.resolve(strict=True)
        info = self.directory.stat()
        if not self.directory.is_dir() or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise ValueError('Native PostgreSQL requires a private owned directory')
        if type(port) is not int or not 1024 <= port <= 65535:
            raise ValueError('Native PostgreSQL requires an unprivileged port')
        self.bin_dir = Path(bin_dir).resolve(strict=True)
        for name in ['initdb', 'postgres']:
            path = self.bin_dir / name
            if not path.is_file() or not os.access(path, os.X_OK):
                raise ValueError('Missing executable in native PostgreSQL binary directory: ' + name)
        self.port = port
        self.data = self.directory / 'postgres-native'
        self.socket = self.directory / 'postgres-socket'
        self.process = None
        self.env = {'PATH': os.defpath, 'LC_ALL': 'C'}

    def start(self):
        if self.process is not None or os.path.lexists(self.data) or os.path.lexists(self.socket):
            raise ValueError('Native PostgreSQL never reuses an existing cluster or socket directory')
        if os.geteuid() == 0:
            raise ValueError('Native PostgreSQL must run as a non-root user')
        version = subprocess.run([str(self.bin_dir / 'postgres'), '--version'],
                                 env=self.env, capture_output=True, text=True, timeout=10, check=True).stdout
        if '(PostgreSQL) 16.' not in version:
            raise ValueError('This native runtime profile requires PostgreSQL 16')
        with (self.directory / 'initdb.log').open('w') as stream:
            result = subprocess.run([str(self.bin_dir / 'initdb'), '-D', str(self.data),
                                     '-U', 'freight_runtime', '-A', 'trust',
                                     '--encoding=UTF8', '--no-locale'],
                                    env=self.env, cwd=self.directory, stdout=stream,
                                    stderr=subprocess.STDOUT, timeout=60)
        if result.returncode != 0:
            raise RuntimeError('Owned native initdb failed; inspect retained initdb.log')
        if (self.data / 'PG_VERSION').read_text().strip() != '16':
            raise RuntimeError('Native initdb and server versions differ')
        self.socket.mkdir(mode=0o700)
        escaped_socket = str(self.socket).replace('\\', '\\\\').replace("'", "''")
        (self.data / 'postgresql.auto.conf').write_text(
            "listen_addresses = '127.0.0.1'\n"
            f"port = {self.port}\n"
            f"unix_socket_directories = '{escaped_socket}'\n"
            "max_connections = 40\nshared_buffers = '64MB'\nssl = off\n"
        )
        with (self.directory / 'postgres-native.log').open('a') as stream:
            self.process = subprocess.Popen([str(self.bin_dir / 'postgres'), '-D', str(self.data)],
                                            env=self.env, cwd=self.directory,
                                            stdout=stream, stderr=subprocess.STDOUT)

    def assert_identity(self, actual_data_directory):
        if self.process is None or self.process.poll() is not None:
            raise RuntimeError('Owned native PostgreSQL process is not running')
        if Path(actual_data_directory).resolve(strict=True) != self.data.resolve(strict=True):
            raise RuntimeError('Refusing an unowned PostgreSQL server on the selected port')
        pid = (self.data / 'postmaster.pid').read_text().splitlines()[0]
        if int(pid) != self.process.pid:
            raise RuntimeError('Native PostgreSQL process identity changed')

    def metadata(self):
        return {'backend': 'native', 'postgres_binary_sha256': hashlib.sha256(
                    (self.bin_dir / 'postgres').read_bytes()).hexdigest(),
                'initdb_binary_sha256': hashlib.sha256((self.bin_dir / 'initdb').read_bytes()).hexdigest(),
                'data_directory': str(self.data), 'listen_address': '127.0.0.1',
                'port': self.port, 'authentication': 'owned-loopback-trust',
                'tls_qualified': False, 'resource_limits_enforced': False,
                'container_qualified': False}

    def stop(self):
        if self.process is None:
            return {'started': False}
        process = self.process
        forced = False
        if process.poll() is None:
            process.send_signal(signal.SIGINT)  # PostgreSQL fast shutdown, aborting clients.
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                forced = True
                process.send_signal(signal.SIGQUIT)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
        self.process = None
        if forced or process.returncode != 0:
            raise RuntimeError('Native PostgreSQL did not stop cleanly; owned data retained')
        with socket.socket() as probe:
            probe.settimeout(1)
            if probe.connect_ex(('127.0.0.1', self.port)) == 0:
                raise RuntimeError('Native PostgreSQL port is still occupied after cleanup')
        return {'started': True, 'fast_shutdown': True, 'forced': False, 'port_closed': True}
