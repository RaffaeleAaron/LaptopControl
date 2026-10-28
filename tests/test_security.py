"""Regression tests use only temporary data, loopback sockets and private IPC."""
import importlib.util
import json
import os
from pathlib import Path
import socket
import stat
import struct
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
import zlib
from png_fixture import write_png

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('camera_security_tests', ROOT/'helpers/camera.py')
camera = importlib.util.module_from_spec(spec)
spec.loader.exec_module(camera)
import security


class RequestsTest(unittest.TestCase):
    def test_valid_pipelining_and_invalid_shapes(self):
        for payload, valid in [(b'{"op":"status"}\n{"op":"list"}\n', True),
                               (b'[]\n', False), (b'{"op":null}\n', False),
                               (b'{"op":"start","value":NaN}\n', False)]:
            a,b=socket.socketpair()
            try:
                a.sendall(payload)
                reader=security.CommandReader(b)
                if valid:
                    self.assertEqual(reader.receive(),{'op':'status'})
                    self.assertEqual(reader.receive(),{'op':'list'})
                else:
                    with self.assertRaises(ValueError):reader.receive()
            finally:a.close();b.close()

    def test_request_size_and_partial_deadline(self):
        a,b=socket.socketpair()
        try:
            a.sendall(b'{'*64)
            with patch.object(security,'MAX_COMMAND_BYTES',32):
                with self.assertRaises(ValueError):security.CommandReader(b).receive()
        finally:a.close();b.close()
        a,b=socket.socketpair()
        try:
            a.sendall(b'{')
            started=time.monotonic()
            with patch.object(security,'REQUEST_SECONDS',.05):
                with self.assertRaises(TimeoutError):security.CommandReader(b).receive()
            self.assertLess(time.monotonic()-started,.5)
        finally:a.close();b.close()

    def test_peer_uid_and_capture_allocation_guard(self):
        a,b=socket.socketpair()
        try:self.assertTrue(security.same_user(b))
        finally:a.close();b.close()
        class WrongUser:
            def getsockopt(self,*args):return struct.pack('iII',123,os.getuid()+1,0)
        self.assertFalse(security.same_user(WrongUser()))
        output=camera.VirtualOutput(lambda:self.fail('Invalid dimensions reached the device'))
        for width,height,fps in [(2**31,720,30),(-1,720,30),(1280,720,1000000),(True,720,30),(1280.,720,30)]:
            with self.assertRaises(ValueError):output.enable(width,height,fps)


class FilesTest(unittest.TestCase):
    def test_private_atomic_json_does_not_follow_predictable_temp_symlink(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);victim=p/'victim';victim.write_text('unchanged')
            directory=p/'config';directory.mkdir()
            (directory/'camera.json.new').symlink_to(victim)
            target=directory/'camera.json';target.symlink_to(victim)
            security.atomic_json(target,{'scenes':[]})
            self.assertEqual(victim.read_text(),'unchanged')
            self.assertEqual(json.loads(target.read_text()),{'scenes':[]})
            self.assertEqual(stat.S_IMODE(target.stat().st_mode),0o600)
            self.assertEqual(stat.S_IMODE(directory.stat().st_mode),0o700)
            linked=p/'linked';linked.symlink_to(directory,target_is_directory=True)
            with self.assertRaises(OSError):security.atomic_json(linked/'other.json',{})

    def test_insecure_runtime_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);p.chmod(0o755)
            with patch.dict(os.environ,{'XDG_RUNTIME_DIR':str(p)}):
                with self.assertRaises(PermissionError):security.runtime_directory()


class AssetBoundaryTest(unittest.TestCase):
    def setUp(self):self.worker=camera.Worker()
    def tearDown(self):self.worker.asset_dir.cleanup()

    def test_valid_png_uses_the_rust_decoder(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d)/'logo.png';write_png(p,16,16)
            result=self.worker.asset({'path':str(p)},1280)
            self.assertEqual((result['width'],result['height']),(16,16))
            data=subprocess.run([camera.raster_binary(),'--tray-icon',result['path']],capture_output=True,check=True).stdout
            self.assertEqual(len(data),4096)
            self.assertTrue(any(data))

    def test_external_svg_resources_and_entities_are_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);write_png(p/'private-neighbor.png')
            for index,value in enumerate(['private-neighbor.png','file:///etc/passwd','https://example.invalid/private.png']):
                svg=p/f'external-{index}.svg'
                svg.write_text(f'<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><image href="{value}" width="16" height="16"/></svg>')
                with self.assertRaisesRegex(ValueError,'external files and URLs'):
                    self.worker.asset({'path':str(svg)},1280)
            svg=p/'entity.svg';svg.write_text('<!DOCTYPE svg [<!ENTITY x SYSTEM "file:///etc/passwd">]><svg xmlns="http://www.w3.org/2000/svg">&x;</svg>')
            with self.assertRaisesRegex(ValueError,'entities, DTDs'):
                self.worker.asset({'path':str(svg)},1280)

    def test_huge_svg_and_raster_are_rejected_before_allocating_canvas(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);svg=p/'large.svg'
            svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1000000000"/>')
            with self.assertRaisesRegex(ValueError,'safe render size'):
                self.worker.asset({'path':str(svg)},1280)
            png=p/'large.png';write_png(png,1,1)
            raw=png.read_bytes();data=struct.pack('>IIBBBBB',100000,100000,8,6,0,0,0)
            chunk=b'IHDR'+data
            png.write_bytes(raw[:8]+struct.pack('>I',13)+chunk+struct.pack('>I',zlib.crc32(chunk))+raw[33:])
            with self.assertRaises(ValueError):self.worker.asset({'path':str(png)},1280)

    def test_sandbox_cannot_read_host_file_or_connect_to_host_listener(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);secret=p/'host-only';secret.write_text('not readable by asset worker')
            output=p/'result';output.touch()
            with socket.socket() as listener:
                listener.bind(('127.0.0.1',0));listener.listen()
                port=listener.getsockname()[1]
                renderer=p/'probe.py'
                renderer.write_text('import pathlib,socket,json\n'+
                    f's=socket.socket();s.settimeout(.2)\nprint(json.dumps({{"host_file":pathlib.Path({str(secret)!r}).exists(),"network":s.connect_ex(("127.0.0.1",{port}))==0}}))\n')
                command,_=security.asset_sandbox(renderer,{'kind':'Text'},output)
                result=subprocess.run(command,capture_output=True,text=True,check=True,timeout=3)
                self.assertEqual(json.loads(result.stdout),{'host_file':False,'network':False})


if __name__=='__main__':unittest.main()
