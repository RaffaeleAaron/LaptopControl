#!/usr/bin/env python3
"""Render application screenshots on an isolated virtual KWin display.
Does not unlock, capture, or inject input into the user's desktop.
"""
import os
import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
ROOT=Path(__file__).resolve().parents[1]
if '--inside' not in sys.argv:
    raise SystemExit(subprocess.call(['dbus-run-session','--',sys.executable,__file__,'--inside']))
with tempfile.TemporaryDirectory(prefix='laptop-control-ui-') as directory:
    runtime=Path(directory);config=runtime/'config';config.mkdir()
    camera_config=config/'laptop-control';camera_config.mkdir()
    (camera_config/'camera.json').write_text(json.dumps({'presets':[{'name':'Office','input':'','controls':{}},{'name':'Backlight','input':'','controls':{}}],'selected_preset':0}))
    kde=Path.home()/'.config/kdeglobals'
    if kde.exists():(config/'kdeglobals').write_bytes(kde.read_bytes())
    env=os.environ.copy();env.update(XDG_RUNTIME_DIR=str(runtime),XDG_CONFIG_HOME=str(config),XDG_CACHE_HOME=str(runtime/'cache'))
    env.pop('WAYLAND_DISPLAY',None);env.pop('WAYLAND_SOCKET',None);env.pop('DISPLAY',None)
    with (runtime/'kwin.log').open('w') as log:
        compositor=subprocess.Popen(['kwin_wayland','--virtual','--no-lockscreen','--no-global-shortcuts','--no-kactivities','--socket','qa-0','--width','1600','--height','1100'],env=env,stdout=log,stderr=log,start_new_session=True)
    try:
        deadline=time.monotonic()+12
        while not (runtime/'qa-0').exists():
            if compositor.poll() is not None or time.monotonic()>deadline:raise RuntimeError((runtime/'kwin.log').read_text()[-4000:])
            time.sleep(.1)
        env['WAYLAND_DISPLAY']='qa-0'
        for name,args in [('studio-scenes',['--demo','--demo-stream','--theme','white']),('studio-capture',['--camera','--theme','white']),('system-monitors',['--demo','--monitors','--demo-cards','--collapsed','--theme','white']),('laptop-control-fans',['--demo','--theme','white']),('camera-save-prompt',['--demo','--camera','--demo-close-prompt','--theme','white']),('settings-credit',['--demo','--settings','--theme','white'])]:
            path=ROOT/('preview-'+name+'.png')
            result=subprocess.run([os.environ.get('LAPTOP_CONTROL_TEST_BINARY',str(ROOT/'target/release/laptop-control')),*args,'--screenshot',str(path)],env=env,capture_output=True,text=True,timeout=20)
            assert result.returncode==0,result.stderr
            assert path.is_file(),name
            print(path,flush=True)
    finally:
        os.killpg(compositor.pid,signal.SIGTERM)
        compositor.wait(timeout=8)
