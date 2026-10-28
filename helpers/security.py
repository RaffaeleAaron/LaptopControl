"""Small Linux-local trust-boundary and resource helpers (no GPU imports)."""
import json
import os
from pathlib import Path
import select
import socket
import stat
import struct
import tempfile
import time

MAX_COMMAND_BYTES = 4 * 1024 * 1024
MAX_CLIENTS = 8
REQUEST_SECONDS = 2.0
CAPTURE_SIZES = {(640, 360), (640, 480), (1280, 720), (1920, 1080)}
CAPTURE_RATES = {10, 15, 20, 30}


def reject_constant(_):
    raise ValueError('Non-finite JSON number')


def capture_size(config):
    if not isinstance(config, dict):
        raise ValueError('Capture configuration must be an object')
    values = tuple(config.get(k) for k in ('width', 'height', 'fps'))
    if any(type(v) is not int for v in values):
        raise ValueError('Capture dimensions and frame rate must be integers')
    width, height, fps = values
    if (width, height) not in CAPTURE_SIZES or fps not in CAPTURE_RATES:
        raise ValueError('Unsupported capture dimensions or frame rate')
    return values


def private_directory(path):
    path = Path(path)
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        if os.fstat(fd).st_uid != os.getuid():
            raise PermissionError('Private directory belongs to another user')
        os.fchmod(fd, 0o700)
    finally:
        os.close(fd)
    return path


def runtime_directory():
    base = Path(os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}'))
    info = base.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise PermissionError('XDG_RUNTIME_DIR must be a private directory owned by this user')
    return private_directory(base / 'laptop-control')


def atomic_json(path, value):
    path = Path(path)
    private_directory(path.parent)
    fd, temporary = tempfile.mkstemp(prefix='.' + path.name + '-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as output:
            json.dump(value, output, ensure_ascii=False, allow_nan=False, indent=2)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def same_user(stream):
    _, uid, _ = struct.unpack('iII', stream.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    return uid == os.getuid()


class CommandReader:
    """Idle subscriptions may wait forever; a partial request has a fixed deadline."""
    def __init__(self, stream):
        self.stream = stream
        self.buffer = bytearray()

    def receive(self):
        deadline = time.monotonic() + REQUEST_SECONDS if self.buffer else None
        while True:
            end = self.buffer.find(b'\n')
            if end >= 0:
                if end + 1 > MAX_COMMAND_BYTES:
                    raise ValueError('Camera request is too large')
                data = bytes(self.buffer[:end + 1])
                del self.buffer[:end + 1]
                value = json.loads(data, parse_constant=reject_constant)
                if not isinstance(value, dict) or not isinstance(value.get('op'), str):
                    raise ValueError('Camera request must be an operation object')
                return value
            if len(self.buffer) >= MAX_COMMAND_BYTES:
                raise ValueError('Camera request is too large')
            if deadline is not None:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not select.select([self.stream], [], [], remaining)[0]:
                    raise TimeoutError('Incomplete camera request timed out')
            chunk = self.stream.recv(min(65536, MAX_COMMAND_BYTES - len(self.buffer)))
            if not chunk:
                return None
            self.buffer.extend(chunk)
            if deadline is None:
                deadline = time.monotonic() + REQUEST_SECONDS


def asset_sandbox(renderer, layer, target, raster=None):
    """Expose only the chosen image, fonts, system libraries and one output file."""
    import copy
    if not Path('/usr/bin/bwrap').is_file():
        raise RuntimeError('Install bubblewrap to render camera overlays safely')
    command = ['/usr/bin/bwrap', '--unshare-all', '--die-with-parent', '--new-session', '--clearenv',
               '--ro-bind', '/usr', '/usr', '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
               '--setenv', 'PATH', '/usr/bin', '--setenv', 'HOME', '/tmp',
               '--setenv', 'LANG', 'C.UTF-8', '--setenv', 'LAPTOP_CONTROL_ASSET_SANDBOX', '1']
    for name in ('/lib', '/lib64', '/bin', '/sbin'):
        path = Path(name)
        if path.is_symlink():
            command += ['--symlink', os.readlink(path), name]
        elif path.exists():
            command += ['--ro-bind', name, name]
    for name in ('/etc/fonts', '/etc/ld.so.cache', '/var/cache/fontconfig'):
        if Path(name).exists():
            command += ['--ro-bind', name, name]
    for i, path in enumerate((Path.home()/'.local/share/fonts', Path.home()/'.fonts')):
        if path.is_dir():
            command += ['--ro-bind', str(path.resolve()), f'/user-fonts-{i}']
    safe_layer = copy.deepcopy(layer)
    if layer.get('kind', 'Image') != 'Text':
        path = Path(layer['path']).resolve()
        mount = '/input' + path.suffix.lower()
        command += ['--ro-bind', str(path), mount]
        safe_layer['path'] = mount
    command += ['--bind', str(target), '/output.png', '--chdir', '/tmp']
    if raster is not None:
        command += ['--ro-bind', str(Path(raster).resolve()), '/raster-render', '--', '/raster-render', '--render-raster', mount, '/output.png']
    else:
        command += ['--ro-bind', str(renderer.resolve()), '/render_asset.py', '--', '/usr/bin/python3', '-I', '/render_asset.py']
    return command, safe_layer
