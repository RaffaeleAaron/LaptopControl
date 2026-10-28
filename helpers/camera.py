#!/usr/bin/env python3
"""Unprivileged JSON bridge: cameractrls controls + Intel GL virtual webcam.
No network, recording, automatic capture, or NVIDIA queries.
"""
import json
import os
from pathlib import Path
import sys
import threading
import subprocess
import tempfile
import hashlib
import copy

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'vendor'))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import cameractrls as cc
from virtual_output import VirtualOutput


def intel_environment():
    for node in sorted(Path('/sys/class/drm').glob('renderD*')):
        if (node / 'device/vendor').read_text().strip() == '0x8086':
            pci = (node / 'device').resolve().name.replace(':', '_').replace('.', '_')
            os.environ['DRI_PRIME'] = 'pci-' + pci
            os.environ['GST_VAAPI_DRM_DEVICE'] = '/dev/dri/' + node.name
            os.environ['GST_VA_DRM_DEVICE'] = '/dev/dri/' + node.name
            os.environ['__NV_PRIME_RENDER_OFFLOAD'] = '0'
            os.environ['__GLX_VENDOR_LIBRARY_NAME'] = 'mesa'
            mesa = '/usr/share/glvnd/egl_vendor.d/50_mesa.json'
            if Path(mesa).exists():
                os.environ['__EGL_VENDOR_LIBRARY_FILENAMES'] = mesa
            os.environ['GST_GL_PLATFORM'] = 'egl'
            return node.name
    raise RuntimeError('Intel render device unavailable; camera effects require the iGPU')


def devices():
    result = []
    for p in sorted(Path('/sys/class/video4linux').glob('video*')):
        path = '/dev/' + p.name
        try:
            caps = cc.get_device_capability(path)
            if b'v4l2loopback' in caps.driver or not (caps.device_caps & cc.V4L2_CAP_VIDEO_CAPTURE):
                continue
            aliases = sorted(Path('/dev/v4l/by-id').glob('*'))
            stable = next((str(alias) for alias in aliases if alias.resolve() == Path(path)), path)
            result.append({'path': stable, 'name': caps.card.decode()})
        except (OSError, ValueError):
            continue
    return result


def output_device():
    for p in Path('/sys/class/video4linux').glob('video*'):
        if (p / 'name').read_text().strip() == 'Laptop Control Camera':
            return '/dev/' + p.name
    return ''


def controls(device, values=None):
    fd = os.open(device, os.O_RDWR | os.O_NONBLOCK)
    try:
        camera = cc.CameraCtrls(device, fd)
        if values:
            errors = []
            camera.setup_ctrls({k: str(v) for k, v in values.items()}, errors)
            if errors:
                raise RuntimeError('; '.join(errors))
            camera = cc.CameraCtrls(device, fd)
        result = []
        for page in camera.get_ctrl_pages():
            for category in page.categories:
                for c in category.ctrls:
                    if c.text_id in ('systemd_cameractrlsd', 'desktop_portal_cameractrlsd'):
                        continue
                    result.append({
                        'id': c.text_id, 'name': c.name, 'kind': c.type, 'value': c.value,
                        'default': c.default, 'min': c.min, 'max': c.max, 'step': c.step,
                        'inactive': c.inactive, 'readonly': c.readonly, 'reopener': c.reopener,
                        'zeroer': c.zeroer, 'unrestorable': c.unrestorable,
                        'group': page.title + ' / ' + category.title, 'help': c.tooltip or '',
                        'menu': [{'id': m.text_id, 'name': m.name} for m in (c.menu or [])],
                    })
        return result
    finally:
        os.close(fd)



def restore_controls(device, saved):
    """Restore auto modes first; inactive manual controls never block streaming."""
    warnings = []
    metadata = {c['id']: c for c in controls(device)}
    automatic = {k: v for k, v in saved.items() if k in metadata and
                 ('auto' in k) and not any(metadata[k][flag] for flag in
                    ('readonly','inactive','reopener','unrestorable','zeroer')) and
                 metadata[k]['kind'] not in ('button','info')}
    if automatic:
        try:
            controls(device, automatic)
        except Exception as error:
            warnings.append(str(error))
    metadata = {c['id']: c for c in controls(device)}
    eligible = {}
    skipped = []
    for key, value in saved.items():
        c = metadata.get(key)
        if key in automatic:
            continue
        if not c or c['inactive'] or c['readonly'] or c['reopener'] or c['unrestorable'] or c['zeroer']:
            skipped.append(key)
        elif c['kind'] not in ('button', 'info'):
            eligible[key] = value
    if eligible:
        try:
            controls(device, eligible)
        except Exception as error:
            warnings.append(str(error))
    if skipped:
        warnings.append('Skipped inactive or unavailable saved controls: ' + ', '.join(skipped))
    return warnings


class Worker:
    def __init__(self):
        self.pipeline = None
        self.config = None
        self.layers = {}
        self.assets = {}
        self.preview = True
        self.preview_lock = threading.Lock()
        self.stdout_lock = threading.Lock()
        self.preview_path = ''
        self.preview_width, self.preview_height = 480, 270
        self.preview_counter = 0
        self.device = ''
        self.gpu = ''
        self.error = None
        self.warnings = []
        self.state = 'Stopped'
        self.Gst = None
        self.virtual = VirtualOutput(output_device)
        self.event_sink = None
        self.service = None
        self.serial = 0
        self.epoch = 0
        self.asset_dir = tempfile.TemporaryDirectory(prefix='laptop-control-assets-', dir=os.environ.get('XDG_RUNTIME_DIR'))

    def send(self, **data):
        if self.event_sink:
            self.event_sink(data)
        else:
            with self.stdout_lock:print(json.dumps(data),flush=True)

    def snapshot(self, include_controls=False, request=None):
        data = {'running': self.state == 'Live', 'state': self.state,
                'output': output_device(), 'gpu': self.gpu, 'device': self.device,
                'error': self.error, 'warnings': self.warnings,
                'virtual_enabled':self.virtual.enabled,
                'virtual_state': 'Live' if self.virtual.enabled and self.state=='Live' else 'Standby' if self.virtual.enabled else 'Off',
                'tray_available': bool(self.service and self.service.tray),
                'tray_bus_name':self.service.tray.bus.get_unique_name() if self.service and self.service.tray else '',
                'preview_width': self.preview_width, 'preview_height': self.preview_height,
                'layers': [{'id': key, 'aspect': layer['asset']['height']/layer['asset']['width']} for key,layer in self.layers.items()]}
        if request:data['request']=request
        if include_controls:
            data['devices'] = devices()
            try:
                data['controls'] = controls(self.device) if self.device else []
            except Exception as error:
                data['controls'] = []
                data['controls_error'] = str(error)
                data['warnings'] = self.warnings + ['Camera controls unavailable: ' + str(error)]
        self.send(**data)

    def init_gst(self):
        if self.Gst:
            return
        if self.service and not os.environ.get('WAYLAND_DISPLAY'):
            # A login service may start before KDE imports its display environment.
            result=subprocess.run(['/usr/bin/systemctl','--user','show-environment'],capture_output=True,text=True,timeout=2)
            for line in result.stdout.splitlines():
                key,_,value=line.partition('=')
                if key in ('WAYLAND_DISPLAY','DISPLAY','XAUTHORITY','XDG_CURRENT_DESKTOP'):os.environ[key]=value
        self.gpu = intel_environment()
        import gi
        gi.require_version('Gst', '1.0')
        from gi.repository import Gst
        Gst.init(None)
        self.Gst = Gst

    def stop(self):
        self.epoch += 1
        if self.pipeline:
            self.pipeline.set_state(self.Gst.State.NULL)
            self.pipeline = None
        with self.preview_lock:
            if self.preview_path:
                Path(self.preview_path).unlink(missing_ok=True)
        self.layers = {}
        self.state = 'Stopped'
        if self.virtual.enabled:self.virtual.standby()

    @staticmethod
    def quote(value):
        return json.dumps(str(value), ensure_ascii=False)

    def asset(self, layer, output_width):
        """Rasterize changed SVG/text once, outside the per-frame GPU pipeline."""
        from PIL import Image
        kind = layer.get('kind', 'Image')
        if kind == 'Text':
            key_data = {k: layer.get(k) for k in ('text', 'font', 'font_size', 'bold', 'color', 'width')}
            key_data['output_width'] = output_width
        else:
            path = Path(layer['path'])
            stat = path.stat()
            key_data = {'path': str(path.resolve()), 'modified': stat.st_mtime_ns, 'size': stat.st_size}
        key = hashlib.sha256(json.dumps(key_data, sort_keys=True).encode()).hexdigest()
        if key in self.assets:
            return self.assets[key]
        target = Path(self.asset_dir.name) / (key + '.png')
        if kind == 'Text':
            import gi
            gi.require_version('Pango', '1.0')
            gi.require_version('PangoCairo', '1.0')
            from gi.repository import Pango, PangoCairo
            import cairo
            width = max(16, round(output_width * float(layer.get('width', .5))))
            scratch = cairo.ImageSurface(cairo.FORMAT_ARGB32, 1, 1)
            layout = PangoCairo.create_layout(cairo.Context(scratch))
            font = Pango.FontDescription()
            font.set_family(layer.get('font', 'Sans'))
            font.set_absolute_size(float(layer.get('font_size', 36)) * Pango.SCALE)
            font.set_weight(Pango.Weight.BOLD if layer.get('bold') else Pango.Weight.NORMAL)
            layout.set_font_description(font)
            layout.set_text(layer.get('text', ''), -1)
            layout.set_width((width - 8) * Pango.SCALE)
            layout.set_wrap(Pango.WrapMode.WORD_CHAR)
            _, height = layout.get_pixel_size()
            height = max(8, height + 8)
            surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, width, height)
            context = cairo.Context(surface)
            context.set_source_rgba(*(float(v) / 255 for v in layer.get('color', [255]*4)))
            context.move_to(4, 4)
            PangoCairo.show_layout(context, layout)
            surface.write_to_png(str(target))
        elif str(path).lower().endswith('.svg'):
            import gi
            gi.require_version('Rsvg', '2.0')
            from gi.repository import Rsvg
            import cairo
            handle = Rsvg.Handle.new_from_file(str(path))
            valid, iw, ih = handle.get_intrinsic_size_in_pixels()
            if not valid:
                dimensions = handle.get_intrinsic_dimensions()
                box = dimensions[-1]
                iw, ih = (box.width, box.height) if dimensions[-2] else (512., 512.)
            width = max(1, min(1280, round(iw)))
            height = max(1, round(width * ih / max(1., iw)))
            surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, width, height)
            viewport = Rsvg.Rectangle()
            viewport.x = viewport.y = 0
            viewport.width, viewport.height = width, height
            handle.render_document(cairo.Context(surface), viewport)
            surface.write_to_png(str(target))
        else:
            with Image.open(path) as image:
                width, height = image.size
                image.convert('RGBA').save(target)
        result = {'key': key, 'path': str(target), 'width': width, 'height': height}
        self.assets[key] = result
        return result

    def remove_layer(self, key):
        old = self.layers.pop(key)
        old['bin'].set_state(self.Gst.State.NULL)
        old['bin'].get_static_pad('src').unlink(old['pad'])
        self.pipeline.get_by_name('mix').release_request_pad(old['pad'])
        self.pipeline.remove(old['bin'])

    def scene(self, config):
        w, h = config['width'], config['height']
        desired = {}
        for i, layer in enumerate(config.get('overlays', [])):
            key = layer.get('id', str(i))
            try:
                asset = self.asset(layer, w)
                desired[key] = (layer, asset, i + 1)
            except Exception as error:
                self.warnings.append('Layer ' + layer.get('name', str(i+1)) + ': ' + str(error))
        for key in list(self.layers):
            if key not in desired or self.layers[key]['asset']['key'] != desired[key][1]['key']:
                self.remove_layer(key)
        mixer = self.pipeline.get_by_name('mix')
        for key, (layer, asset, zorder) in desired.items():
            if key not in self.layers:
                self.serial += 1
                source = self.Gst.parse_bin_from_description(
                    f'filesrc location={self.quote(asset["path"])} ! pngdec ! videoconvert ! video/x-raw,format=RGBA ! glupload ! glcolorconvert ! imagefreeze is-live=true', True)
                source.set_name('layer_' + str(self.serial))
                self.pipeline.add(source)
                pad = mixer.request_pad_simple('sink_%u')
                if source.get_static_pad('src').link(pad) != self.Gst.PadLinkReturn.OK:
                    raise RuntimeError('Could not connect layer to the Intel compositor')
                self.layers[key] = {'bin': source, 'pad': pad, 'asset': asset}
                source.sync_state_with_parent()
            pad = self.layers[key]['pad']
            width = max(1, min(w, round(w * float(layer['width']))))
            height = max(1, min(h, round(width * asset['height'] / asset['width'])))
            pad.set_property('width', width)
            pad.set_property('height', height)
            pad.set_property('xpos', round(float(layer['x']) * (w-width)))
            pad.set_property('ypos', round(float(layer['y']) * (h-height)))
            pad.set_property('alpha', float(layer['opacity']) if layer['enabled'] else 0.)
            pad.set_property('zorder', zorder)
        used = {value[1]['key'] for value in desired.values()}
        for key in list(self.assets):
            if key not in used:
                Path(self.assets.pop(key)['path']).unlink(missing_ok=True)
        self.config = copy.deepcopy(config)

    def start(self, config, test=False):
        self.init_gst()
        self.stop()
        self.state = 'Starting'
        self.snapshot()
        output = output_device()
        w, h, fps = int(config['width']), int(config['height']), int(config['fps'])
        if (w, h) not in [(640,480),(640,360),(1280,720),(1920,1080)] or fps not in (10,15,20,30):
            raise ValueError('Unsupported capture dimensions or frame rate')
        source = config['input']
        if not test:
            if not source:source=next((d['path'] for d in devices() if 'IR' not in d['name']),'')
            source = next((d['path'] for d in devices() if Path(d['path']).resolve() == Path(source).resolve()), None)
            if not source:
                raise ValueError('Select an available physical camera')
            config = copy.deepcopy(config)
            config['input'] = source
            self.device = source
            self.warnings += restore_controls(source, config.get('controls', {}))
        if test:
            src = f'videotestsrc is-live=true pattern=smpte ! video/x-raw,width={w},height={h},framerate={fps}/1'
        elif config['format'] == 'MJPG':
            decoder = 'vajpegdec' if self.Gst.ElementFactory.find('vajpegdec') else 'jpegdec'
            src = f'v4l2src device={self.quote(source)} do-timestamp=true ! image/jpeg,width={w},height={h},framerate=30/1 ! {decoder}'
            if fps != 30:
                src += f' ! videorate drop-only=true ! video/x-raw,framerate={fps}/1'
        else:
            src = f'v4l2src device={self.quote(source)} do-timestamp=true ! video/x-raw,format=YUY2,width={w},height={h},framerate={fps}/1'
        self.preview_width = min(480, w)
        self.preview_height = round(self.preview_width*h/w)
        pipeline = (
            f'glvideomixer name=mix background=black latency=0 ! video/x-raw(memory:GLMemory),width={w},height={h},framerate={fps}/1 ! tee name=out '
            f'out. ! queue max-size-buffers=2 leaky=downstream ! valve name=virtual_gate drop=true ! glcolorconvert ! video/x-raw(memory:GLMemory),format=YUY2 ! gldownload ! video/x-raw,format=YUY2 ! appsink name=virtual emit-signals=true sync=false async=false max-buffers=1 drop=true '
            'out. ! queue max-size-buffers=1 leaky=downstream ! valve name=preview_gate drop=true ! '
            f'glcolorscale ! video/x-raw(memory:GLMemory),format=RGBA,width={self.preview_width},height={self.preview_height},framerate={fps}/1 ! gldownload ! video/x-raw,format=RGBA ! '
            'appsink name=preview emit-signals=true sync=false async=false max-buffers=1 drop=true '
            f'{src} ! glupload ! glcolorconvert ! glvideoflip method={"horizontal-flip" if config["mirror"] else "none"} ! mix.sink_0 '
        )
        self.pipeline = self.Gst.parse_launch(pipeline)
        mixer = self.pipeline.get_by_name('mix')
        if mixer.find_property('ignore-inactive-pads'):
            mixer.set_property('ignore-inactive-pads', True)
        self.scene(config)
        self.pipeline.get_by_name('preview').connect('new-sample', self.frame)
        self.pipeline.get_by_name('virtual').connect('new-sample', self.virtual_frame)
        self.pipeline.get_by_name('virtual_gate').set_property('drop',not self.virtual.enabled)
        bus = self.pipeline.get_bus()
        bus.add_signal_watch()
        bus.connect('message', self.bus_message)
        self.pipeline.get_by_name('preview_gate').set_property('drop', not self.preview)
        if self.pipeline.set_state(self.Gst.State.PLAYING) == self.Gst.StateChangeReturn.FAILURE:
            raise RuntimeError('Camera pipeline could not start')
        epoch = self.epoch
        def deadline():
            if self.epoch == epoch and self.state == 'Starting':
                self.stop()
                self.state = 'Error'
                self.error = 'Camera startup timed out. Close other camera apps and retry.'
                self.snapshot()
            return False
        from gi.repository import GLib
        GLib.timeout_add_seconds(8, deadline)

    def bus_message(self, bus, message):
        if not self.pipeline or bus != self.pipeline.get_bus():
            return
        if message.type == self.Gst.MessageType.ERROR:
            error, debug = message.parse_error()
            print(debug, file=sys.stderr)
            self.stop()
            self.state = 'Error'
            self.error = str(error)
            self.snapshot()
        elif message.type == self.Gst.MessageType.STATE_CHANGED and message.src == self.pipeline:
            old, new, pending = message.parse_state_changed()
            if new == self.Gst.State.PLAYING:
                self.state = 'Live'
                self.snapshot()

    def virtual_frame(self,sink):
        sample=sink.emit('pull-sample')
        if sample and self.virtual.enabled:
            buffer=sample.get_buffer();ok,mapped=buffer.map(self.Gst.MapFlags.READ)
            if ok:
                try:self.virtual.write(mapped.data)
                except OSError as error:
                    from gi.repository import GLib
                    GLib.idle_add(self.output_error,str(error))
                finally:buffer.unmap(mapped)
        return self.Gst.FlowReturn.OK

    def output_error(self,error):
        self.virtual.close()
        if self.pipeline:self.pipeline.get_by_name('virtual_gate').set_property('drop',True)
        self.error='Virtual output: '+error;self.snapshot();return False

    def standby_tick(self):
        if self.virtual.enabled and self.state!='Live':
            try:self.virtual.standby()
            except OSError as error:self.output_error(str(error))
        return True

    def frame(self, sink):
        sample = sink.emit('pull-sample')
        with self.preview_lock:
            if self.preview and self.preview_path and sample:
                buffer = sample.get_buffer()
                ok, mapped = buffer.map(self.Gst.MapFlags.READ)
                if ok:
                    try:
                        path = Path(self.preview_path)
                        temp = Path(self.preview_path + '.new')
                        temp.write_bytes(mapped.data)
                        temp.replace(path)
                        self.preview_counter += 1
                        self.send(frame=self.preview_counter, width=self.preview_width, height=self.preview_height)
                    finally:
                        buffer.unmap(mapped)
        return self.Gst.FlowReturn.OK

    def handle(self, command):
        try:
            op = command['op']
            self.error = None
            if op == 'list':
                available = devices()
                wanted = command.get('device') or self.device
                self.device = next((d['path'] for d in available if Path(d['path']).resolve() == Path(wanted).resolve()), '') if wanted else ''
                if not self.device:
                    self.device = next((d['path'] for d in available if 'IR' not in d['name']), available[0]['path'] if available else '')
                self.snapshot(True)
            elif op == 'read_controls':
                if not self.device:raise RuntimeError('Select a camera before saving hardware settings')
                self.snapshot(True,command.get('request'))
            elif op == 'apply_preset':
                if not self.device:raise RuntimeError('Select a camera before loading a preset')
                self.warnings=restore_controls(self.device,command['values'])
                if self.config:self.config['controls']=copy.deepcopy(command['values'])
                self.snapshot(True,command.get('request'))
            elif op == 'set':
                metadata = {c['id']:c for c in controls(self.device)}
                values = {k:v for k,v in command['values'].items() if k in metadata and not metadata[k]['inactive'] and not metadata[k]['readonly']}
                if self.pipeline and any(metadata[k]['reopener'] for k in values):
                    raise RuntimeError('Stop the camera before loading presets or changing capture format')
                if values:
                    controls(self.device, values)
                self.snapshot(True)
            elif op == 'virtual':
                if command['enabled']:
                    c=command.get('config') or self.config or {'width':1280,'height':720,'fps':30}
                    self.virtual.enable(c['width'],c['height'],c['fps'])
                else:self.virtual.close()
                if self.pipeline:self.pipeline.get_by_name('virtual_gate').set_property('drop',not self.virtual.enabled)
                if self.service:self.service.persist_output(self.virtual.enabled)
                self.snapshot()
            elif op == 'start':
                self.warnings = []
                if self.virtual.enabled:
                    c=command['config'];self.virtual.enable(c['width'],c['height'],c['fps'])
                self.start(command['config'], command.get('test', False))
                self.snapshot()
            elif op == 'scene':
                self.warnings = []
                if not self.pipeline:self.config=copy.deepcopy(command['config'])
                if self.pipeline:
                    old, new = self.config, command['config']
                    if any(old[k] != new[k] for k in ('input','width','height','fps','format','mirror')):
                        raise RuntimeError('Stop the camera before changing capture configuration')
                    self.scene(new)
                self.snapshot()
            elif op == 'preview':
                with self.preview_lock:
                    self.preview = command['enabled']
                    self.preview_path = command['path']
                    if not self.preview:
                        Path(self.preview_path).unlink(missing_ok=True)
                if self.pipeline:
                    self.pipeline.get_by_name('preview_gate').set_property('drop', not self.preview)
            elif op == 'stop':
                self.stop()
                self.snapshot()
            elif op == 'pick_logo':
                def pick():
                    result = subprocess.run(['/usr/bin/kdialog','--getopenfilename',str(Path.home()),'*.svg *.png|SVG and PNG logos'],capture_output=True,text=True)
                    if result.returncode == 0 and result.stdout.strip():
                        self.send(logo=result.stdout.strip())
                threading.Thread(target=pick, daemon=True).start()
            elif op == 'quit':
                self.stop()
                self.virtual.close()
                self.loop.quit()
            else:
                raise ValueError('Unknown camera operation')
        except Exception as error:
            if self.state == 'Starting':
                self.stop()
                self.state = 'Error'
            self.error = str(error)
            if command.get('request'):self.send(request=command['request'],controls_error=self.error)
            self.snapshot()
        return False

    def run(self, service=False, gui=None, fan_socket='/run/raf-fans/control.sock'):
        from gi.repository import GLib
        self.loop = GLib.MainLoop()
        GLib.timeout_add_seconds(1,self.standby_tick)
        if service:
            from control_service import ControlService
            try:
                self.service=ControlService(self,gui,fan_socket)
                self.loop.run()
            except BlockingIOError:pass  # An existing instance already owns the socket.
            finally:
                self.stop();self.virtual.close()
                if self.service:self.service.close()
                self.asset_dir.cleanup()
            return
        def read_commands():
            for line in sys.stdin:
                try:
                    GLib.idle_add(self.handle, json.loads(line))
                except (ValueError, TypeError) as error:
                    self.send(error=str(error))
            GLib.idle_add(self.handle, {'op':'quit'})
        threading.Thread(target=read_commands, daemon=True).start()
        try:
            self.loop.run()
        finally:
            self.stop()
            self.virtual.close()
            self.asset_dir.cleanup()


if __name__ == '__main__':
    import argparse
    parser=argparse.ArgumentParser()
    parser.add_argument('--service',action='store_true')
    parser.add_argument('--gui',default=str(Path.home()/'.local/bin/laptop-control'))
    parser.add_argument('--fan-socket',default='/run/raf-fans/control.sock')
    args=parser.parse_args()
    Worker().run(args.service,args.gui,args.fan_socket)
