"""Per-user camera/tray service and local JSON IPC, independent of GUI lifetime."""
import fcntl
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import threading
import time
import stat
from gi.repository import GLib
from security import CommandReader, MAX_CLIENTS, MAX_COMMAND_BYTES, atomic_json, runtime_directory, same_user

class Client:
    def __init__(self, stream):
        self.stream=stream;self.subscribed=False;self.lock=threading.Lock()
    def send(self, data):
        try:
            with self.lock:self.stream.sendall((json.dumps(data)+'\n').encode(),socket.MSG_DONTWAIT)
            return True
        except (OSError,ValueError):return False
    def close(self):
        try:self.stream.shutdown(socket.SHUT_RDWR)
        except OSError:pass
        self.stream.close()

class ControlService:
    def __init__(self,worker,gui,fan_socket):
        self.worker,self.gui=worker,gui
        self.runtime=runtime_directory()
        self.lock_file=os.fdopen(os.open(self.runtime/'camera.lock',os.O_CREAT|os.O_WRONLY|os.O_NOFOLLOW|os.O_CLOEXEC,0o600),'w')
        fcntl.flock(self.lock_file,fcntl.LOCK_EX|fcntl.LOCK_NB)
        self.path=self.runtime/'camera.sock'
        if self.path.exists() or self.path.is_symlink():
            info=self.path.lstat()
            if not stat.S_ISSOCK(info.st_mode) or info.st_uid!=os.getuid():
                raise PermissionError('Refusing to replace an unexpected camera socket path')
            self.path.unlink()
        self.listener=socket.socket(socket.AF_UNIX);self.listener.bind(str(self.path));os.chmod(self.path,0o600);self.listener.listen(8)
        self.clients=[];self.current=None;self.last_launch=0.;self.closed=False
        self.clients_lock=threading.Lock()
        self.directory=Path(os.environ.get('XDG_CONFIG_HOME',str(Path.home()/'.config')))/'laptop-control'
        self.book_cache=None;self.book_stamp=None;self.live_book=None
        self.tray=None
        worker.service=self
        worker.event_sink=self.event
        self.thread=threading.Thread(target=self.accept,daemon=True);self.thread.start()
        from tray import Tray
        try:self.tray=Tray(self,fan_socket)
        except Exception as error:worker.warnings.append('Tray unavailable: '+str(error))
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT,signal.SIGTERM,self.quit)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT,signal.SIGINT,self.quit)
        self.restore_virtual()

    def load_book(self):
        path=self.directory/'camera.json'
        try:
            stamp=path.stat().st_mtime_ns
            if self.book_cache is not None and stamp==self.book_stamp:return self.book_cache
            if path.stat().st_size>MAX_COMMAND_BYTES:raise ValueError('Camera configuration is too large')
            book=json.loads(path.read_text())
            if not isinstance(book,dict):raise ValueError('Invalid camera configuration')
            scenes=book.get('scenes')
            if scenes is not None and (not isinstance(scenes,list) or any(not isinstance(s,dict) or not isinstance(s.get('layers',[]),list) for s in scenes)):
                raise ValueError('Invalid scene list')
            if type(book.get('selected_scene',0)) is not int:raise ValueError('Invalid scene selection')
        except (OSError,ValueError):book={};stamp=None
        defaults={'input':'','width':1280,'height':720,'fps':30,'format':'MJPG','mirror':False,'controls':{},'selected_scene':0}
        for key,value in defaults.items():book.setdefault(key,value)
        if not book.get('scenes'):book['scenes']=[{'name':'Scene 1','layers':book.pop('overlays',[])}]
        for i,scene in enumerate(book['scenes']):
            scene.setdefault('layers',[])
            if not isinstance(scene.get('name'),str):scene['name']=f'Scene {i+1}'
        book['selected_scene']=min(max(0,book['selected_scene']),len(book['scenes'])-1)
        self.book_cache,self.book_stamp=book,stamp
        return book

    @staticmethod
    def payload(book):
        index=book['selected_scene']
        return {**{key:book[key] for key in ('input','width','height','fps','format','mirror','controls')},'overlays':book['scenes'][index]['layers'],'scene_index':index}

    def persist_output(self,enabled):
        atomic_json(self.directory/'virtual-output.json',{'enabled':enabled})

    def restore_virtual(self):
        try:enabled=json.loads((self.directory/'virtual-output.json').read_text()).get('enabled',True)
        except (OSError,ValueError,AttributeError):enabled=True
        if enabled:self.worker.handle({'op':'virtual','enabled':True,'config':self.payload(self.load_book())})

    def accept(self):
        while not self.closed:
            try:stream,_=self.listener.accept()
            except OSError:break
            with self.clients_lock:
                if len(self.clients)>=MAX_CLIENTS or not same_user(stream):
                    stream.close();continue
                client=Client(stream);self.clients.append(client)
            threading.Thread(target=self.read,args=(client,),daemon=True).start()

    def read(self,client):
        try:
            reader=CommandReader(client.stream)
            while not self.closed:
                command=reader.receive()
                if command is None:break
                done=threading.Event()
                def dispatch_one(command=command,done=done):
                    try:return self.dispatch(client,command)
                    finally:done.set()
                GLib.idle_add(dispatch_one)
                while not self.closed and not done.wait(.2):pass
        except (OSError,ValueError,TypeError,RecursionError):pass
        finally:GLib.idle_add(self.disconnected,client)

    def disconnected(self,client):
        with self.clients_lock:
            if client in self.clients:self.clients.remove(client)
        client.close()
        if not any(c.subscribed for c in self.clients):self.worker.handle({'op':'preview','enabled':False,'path':str(self.runtime/'preview.rgba')})
        return False

    def event(self,data):
        targets=[c for c in list(self.clients) if c.subscribed or (c==self.current and 'frame' not in data)]
        for client in targets:
            if not client.send(data):GLib.idle_add(self.disconnected,client)
        if 'frame' not in data and self.tray:self.tray.changed()

    def dispatch(self,client,command):
        self.current=client
        try:
            op=command.get('op')
            if op=='subscribe':
                client.subscribed=True;self.worker.snapshot()
                if self.worker.state=='Live' and self.live_book:client.send({'live_book':self.live_book})
            elif op=='show_existing':client.send({'shown':self.show_gui(launch=False,page=command.get('page'),token=command.get('token',''))})
            elif op=='show_gui':self.show_gui(page=command.get('page'));client.send({'ok':True})
            elif op=='reload_scenes':
                self.book_cache=None
                if self.worker.state=='Live':self.live_book=command.get('book') or self.load_book()
                self.worker.snapshot()
            elif op=='status':self.worker.snapshot()
            elif op=='preview':
                command['path']=str(self.runtime/'preview.rgba');self.worker.handle(command)
            elif op=='quit':self.quit()
            else:
                self.worker.handle(command)
                if not self.worker.error and op in ('start','scene') and command.get('book'):self.live_book=command['book']
                if op=='stop':self.live_book=None
        except (ValueError,TypeError,KeyError,IndexError,AttributeError) as error:
            client.send({'error':'Invalid camera request: '+str(error)})
        finally:self.current=None
        return False

    def show_gui(self,launch=True,page=None,token=''):
        subscribers=[c for c in self.clients if c.subscribed]
        if subscribers:
            shown=False
            for client in subscribers:
                if client.send({'action':'show_gui','page':page,'token':token}):shown=True
                else:self.disconnected(client)
            if shown:return True
        if launch and time.monotonic()-self.last_launch>2:
            self.last_launch=time.monotonic()
            environment=os.environ.copy()
            # The login service can precede KDE's import of the display variables.
            try:
                result=subprocess.run(['/usr/bin/systemctl','--user','show-environment'],capture_output=True,text=True,timeout=2)
                for line in result.stdout.splitlines():
                    key,_,value=line.partition('=')
                    if key in ('WAYLAND_DISPLAY','DISPLAY','XAUTHORITY','XDG_CURRENT_DESKTOP','XDG_SESSION_TYPE'):environment[key]=value
            except (OSError,subprocess.TimeoutExpired):pass
            if token:environment['XDG_ACTIVATION_TOKEN']=token
            args=[self.gui]
            if page=='camera':args.append('--camera')
            subprocess.Popen(args,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env=environment,start_new_session=True)
        return False

    def select_scene(self,index):
        book=self.load_book()
        if not 0<=index<len(book['scenes']):return
        book['selected_scene']=index
        atomic_json(self.directory/'camera.json',book)
        self.book_stamp=(self.directory/'camera.json').stat().st_mtime_ns
        self.worker.handle({'op':'scene','config':self.payload(book)})
        if not self.worker.error:self.live_book=book
        self.event({'action':'selected_scene','index':index})

    def quit(self):
        self.event({'action':'quit_gui'})
        self.worker.loop.quit();return False

    def request_quit(self):
        # Keep capture and the service alive while the GUI asks about unsaved edits.
        if any(c.subscribed for c in self.clients):self.event({'action':'request_quit'})
        else:self.quit()

    def close(self):
        self.closed=True;self.listener.close()
        for client in list(self.clients):client.close()
        self.path.unlink(missing_ok=True)
        self.lock_file.close()
