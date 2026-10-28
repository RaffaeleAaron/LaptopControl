#!/usr/bin/env python3
"""Host integration: private D-Bus, saved scenes, simulated fans, real virtual output.
No physical camera is opened. Run serially with the camera smoke test.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

ROOT=Path(__file__).resolve().parents[1]
if '--inside' not in sys.argv:
    raise SystemExit(subprocess.call(['dbus-run-session','--',sys.executable,__file__,'--inside']))
sys.path.insert(0,str(ROOT/'helpers'))
import camera
from gi.repository import Gio, GLib

def wait(predicate,seconds=10):
    end=time.monotonic()+seconds
    while time.monotonic()<end:
        result=predicate()
        if result:return result
        time.sleep(.05)
    raise AssertionError('Timed out')

with tempfile.TemporaryDirectory(prefix='laptop-control-service-') as directory:
    directory=Path(directory);config=directory/'config/laptop-control';config.mkdir(parents=True)
    book={'input':'','width':640,'height':360,'fps':30,'format':'MJPG','mirror':False,'controls':{},'selected_scene':0,'scenes':[{'name':'Meeting','layers':[]},{'name':'Presentation','layers':[]}]}
    (config/'camera.json').write_text(json.dumps(book))
    env=os.environ.copy()
    if env.get('WAYLAND_DISPLAY') and not env['WAYLAND_DISPLAY'].startswith('/'):
        env['WAYLAND_DISPLAY']=str(Path(env['XDG_RUNTIME_DIR'])/env['WAYLAND_DISPLAY'])
    env.update(XDG_RUNTIME_DIR=str(directory),XDG_CONFIG_HOME=str(directory/'config'))
    fan_socket=directory/'fan.sock'
    (directory/'fans').mkdir()
    fan_binary=os.environ.get('LAPTOP_CONTROL_FAN_TEST_BINARY',str(ROOT/'target/debug/raf-fand'))
    fan=subprocess.Popen([fan_binary,'--simulate','--socket',str(fan_socket),'--simulate-state-dir',str(directory/'fans')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    service=None
    log=(directory/'service.log').open('w+')
    def launch():
        return subprocess.Popen(['/usr/bin/python3',str(ROOT/'helpers/camera.py'),'--service','--gui','/bin/true','--fan-socket',str(fan_socket)],env=env,stdout=log,stderr=log)
    path=directory/'laptop-control/camera.sock'
    def request(command):
        with socket.socket(socket.AF_UNIX) as stream:
            stream.settimeout(5);stream.connect(str(path));stream.sendall((json.dumps(command)+'\n').encode())
            with stream.makefile('r') as reader:return json.loads(reader.readline())
    def status():
        try:return request({'op':'status'})
        except (OSError,ValueError):return {}
    def call(method,args,signature=None,interface='com.canonical.dbusmenu',object_path='/MenuBar'):
        return bus.call_sync(name,object_path,interface,method,GLib.Variant(signature,args) if signature else None,None,Gio.DBusCallFlags.NONE,4000,None).unpack()
    def event(number):call('Event',(number,'clicked',GLib.Variant('i',0),0),'(isvu)')
    try:
        wait(fan_socket.exists);service=launch()
        state=wait(lambda:(s if (s:=status()).get('virtual_enabled') else None))
        assert state['state']=='Stopped' and state['tray_available'],state
        assert camera.cc.get_device_capability(camera.output_device()).device_caps & camera.cc.V4L2_CAP_VIDEO_CAPTURE
        targets=[]
        for fd in Path(f'/proc/{service.pid}/fd').iterdir():
            try:targets.append(os.readlink(fd))
            except FileNotFoundError:pass
        assert not any('/dev/dri/' in p or (p.startswith('/dev/video') and p!=camera.output_device()) for p in targets),targets
        bus=Gio.bus_get_sync(Gio.BusType.SESSION,None);name=state['tray_bus_name']
        layout=call('GetLayout',(0,-1,[]),'(iias)')[1]
        def flatten(node):
            return [node]+[item for child in node[2] for item in flatten(child)]
        nodes={node[0]:node for node in flatten(layout)}
        assert [nodes[i][1]['label'] for i in (10,11,12,13,20,1000,1001)]==['Fan Control','Firmware','Office','Gaming','Webcam','Meeting','Presentation']
        for number,profile in [(12,'Office'),(13,'Gaming'),(11,'Firmware')]:
            event(number)
            menu=call('GetLayout',(10,-1,[]),'(iias)')[1]
            assert next(c for c in menu[2] if c[0]==number)[1]['toggle-state']==1,(profile,menu,status())
        event(1001);assert json.loads((config/'camera.json').read_text())['selected_scene']==1
        client=socket.socket(socket.AF_UNIX);client.settimeout(5);client.connect(str(path))
        client.sendall(b'{"op":"subscribe"}\n');reader=client.makefile('r');json.loads(reader.readline())
        call('Activate',(0,0),'(ii)',interface='org.kde.StatusNotifierItem',object_path='/StatusNotifierItem')
        assert json.loads(reader.readline())['action']=='show_gui'
        event(31)
        assert json.loads(reader.readline())['action']=='request_quit'
        assert status()['virtual_enabled'],'Tray Quit stopped capture before GUI confirmation'
        reader.close();client.close();time.sleep(.1)
        assert status()['virtual_enabled'],'Closing GUI disabled virtual output'
        request({'op':'start','config':book|{'overlays':[]},'book':book,'test':True})
        wait(lambda:status().get('state')=='Live')
        client=socket.socket(socket.AF_UNIX);client.settimeout(5);client.connect(str(path))
        client.sendall(b'{"op":"subscribe"}\n');reader=client.makefile('r')
        assert json.loads(reader.readline())['running']
        assert json.loads(reader.readline())['live_book']==book,'Reopened GUI lost the active scene book'
        reader.close();client.close()
        result=subprocess.run(['v4l2-ctl','-d',camera.output_device(),'--stream-mmap','--stream-count=5','--stream-to=/dev/null'],capture_output=True,text=True,timeout=8)
        assert result.returncode==0,result.stderr
        request({'op':'stop'});assert status()['virtual_state']=='Standby'
        request({'op':'virtual','enabled':False});assert not status()['virtual_enabled']
        service.terminate();service.wait(timeout=8)
        assert json.loads((config/'virtual-output.json').read_text())=={'enabled':False}
        service=launch();wait(lambda:path.exists() and status())
        assert not status()['virtual_enabled'],'Disabled output was enabled on restart'
        print('Tray activation, profile menus, saved scenes, GUI reconnect, persistent standby, synthetic output, no idle GPU/camera: PASS')
    except Exception:
        log.flush();print((directory/'service.log').read_text()[-5000:],file=sys.stderr);raise
    finally:
        if service and service.poll() is None:service.terminate();service.wait(timeout=8)
        fan.terminate();fan.wait(timeout=8);log.close()
