#!/usr/bin/env python3
"""Host tests: synthetic by default; --real opens the camera and discards frames."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'helpers'))
import camera

def main():
    real='--real' in sys.argv
    preview=Path(os.environ['XDG_RUNTIME_DIR'])/'laptop-control-smoke.rgba'
    with tempfile.TemporaryDirectory() as directory:
        logs=Path(directory)/'events.jsonl';errors=Path(directory)/'stderr'
        with logs.open('w') as output, errors.open('w') as stderr:
            helper=Path.home()/'.local/share/laptop-control/camera.py' if '--installed' in sys.argv else ROOT/'helpers/camera.py'
            child=subprocess.Popen(['/usr/bin/python3',str(helper)],stdin=subprocess.PIPE,stdout=output,stderr=stderr,text=True)
        def send(command):child.stdin.write(json.dumps(command)+'\n');child.stdin.flush()
        def messages():
            lines=logs.read_text().splitlines();result=[]
            for line in lines:
                try:result.append(json.loads(line))
                except ValueError:pass
            return result
        def wait(predicate,seconds=10):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                events=messages();failures=[v['error'] for v in events if v.get('error')]
                assert not failures,failures
                if predicate(events):return events
                time.sleep(.05)
            raise AssertionError('Timed out: '+logs.read_text()[-1800:]+'\n'+errors.read_text()[-1800:])
        config={'input':next((d['path'] for d in camera.devices() if 'IR' not in d['name']),''),'width':1280 if real else 640,'height':720 if real else 360,'fps':30,'format':'MJPG','mirror':False,'controls':{},'overlays':[{'id':'png','path':str(ROOT/'assets/laptop-control.png'),'x':1.,'y':0.,'width':.2,'opacity':.8,'enabled':True}]}
        if real:
            saved=Path.home()/'.config/laptop-control/camera.json'
            if saved.exists():config['controls']=json.loads(saved.read_text()).get('controls',{})
        try:
            send({'op':'preview','enabled':not real,'path':str(preview)})
            if real:send({'op':'virtual','enabled':True,'config':config})
            send({'op':'start','config':config,'test':not real})
            events=wait(lambda events:any(e.get('state')=='Live' for e in events))
            targets=[]
            for fd in Path(f'/proc/{child.pid}/fd').iterdir():
                try:
                    target=os.readlink(fd)
                    if '/dev/dri/' in target:targets.append(target)
                except FileNotFoundError:pass
            assert targets and set(targets)=={'/dev/dri/renderD128'},targets
            if real:
                caps=camera.cc.get_device_capability(camera.output_device())
                assert caps.device_caps & camera.cc.V4L2_CAP_VIDEO_CAPTURE,'Virtual camera not advertised as capture'
                result=subprocess.run(['v4l2-ctl','-d',camera.output_device(),'--stream-mmap','--stream-count=5','--stream-to=/dev/null'],capture_output=True,text=True,timeout=6)
                assert result.returncode==0,result.stderr
                metadata=camera.controls(config['input'])
                brightness=next((c for c in metadata if c['id']=='brightness' and not c['inactive'] and not c['readonly']),None)
                if brightness:
                    before=sum(e.get('state')=='Starting' for e in messages())
                    send({'op':'apply_preset','values':{'brightness':str(brightness['value'])},'request':'apply_camera_preset'})
                    wait(lambda events:any(e.get('request')=='apply_camera_preset' and e.get('state')=='Live' for e in events))
                    send({'op':'read_controls','request':'save_camera'})
                    wait(lambda events:any(e.get('request')=='save_camera' and e.get('controls') for e in events))
                    assert sum(e.get('state')=='Starting' for e in messages())==before,'Live preset reopened capture'
                print('Saved control restoration, capture enumeration, live preset/save controls, and real camera output: PASS')
            else:
                wait(lambda events:any(e.get('frame',0)>4 for e in events))
                assert preview.stat().st_size==480*270*4
                first=max(e.get('frame',0) for e in messages());time.sleep(1.1)
                delta=max(e.get('frame',0) for e in messages())-first
                assert 23<=delta<=40,('30 fps preview',delta)
                svg=Path(directory)/'logo.svg';svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50"><rect width="100" height="50" fill="red"/></svg>')
                config['overlays'] += [dict(id='svg',kind='Image',name='SVG logo',path=str(svg),x=0.,y=0.,width=.2,opacity=1.,enabled=True),dict(id='text',kind='Text',name='Caption',text='Laptop Control · Live',font='Sans',font_size=28,color=[255]*4,bold=True,x=0.,y=.9,width=.8,opacity=1.,enabled=True)]
                send({'op':'scene','config':config})
                wait(lambda events:any(len(e.get('layers',[]))==3 and e.get('state')=='Live' for e in events))
                config['overlays']=[];send({'op':'scene','config':config})
                wait(lambda events:any(e.get('layers')==[] and e.get('state')=='Live' for e in events))
                assert sum(e.get('state')=='Starting' for e in messages())==2,'Scene edits restarted the camera'
                send({'op':'preview','enabled':False,'path':str(preview)});time.sleep(.2);assert not preview.exists()
                print('PNG + SVG + text scenes, dynamic layer changes, 30 fps preview, Intel GPU: PASS')
            send({'op':'stop'})
            wait(lambda events:any(e.get('state')=='Stopped' for e in events))
        finally:
            send({'op':'quit'});child.wait(timeout=8);preview.unlink(missing_ok=True)
            diagnostics=errors.read_text()
            if diagnostics:print(diagnostics,file=sys.stderr)
        assert child.returncode==0

if __name__=='__main__':main()
