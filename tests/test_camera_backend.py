import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
spec = importlib.util.spec_from_file_location('camera', Path(__file__).resolve().parents[1] / 'helpers/camera.py')
camera = importlib.util.module_from_spec(spec)
spec.loader.exec_module(camera)

class CameraControlsTest(unittest.TestCase):
    def metadata(self, key, inactive=False):
        return {'id':key,'kind':'integer','inactive':inactive,'readonly':False,
                'reopener':False,'unrestorable':False,'zeroer':False}
    def test_inactive_controls_do_not_prevent_startup(self):
        writes=[]
        def controls(device, values=None):
            if values: writes.append(values)
            return [self.metadata('auto_exposure'), self.metadata('exposure_time_absolute',True),
                    self.metadata('white_balance_automatic'),self.metadata('white_balance_temperature',True),self.metadata('brightness')]
        with patch.object(camera,'controls',controls):
            warnings=camera.restore_controls('/dev/test',{'auto_exposure':'aperture_priority_mode','exposure_time_absolute':'156','white_balance_automatic':'1','white_balance_temperature':'4600','brightness':'128'})
        self.assertEqual(writes,[{'auto_exposure':'aperture_priority_mode','white_balance_automatic':'1'},{'brightness':'128'}])
        self.assertIn('exposure_time_absolute',warnings[0])
    def test_saved_control_error_is_nonfatal(self):
        def controls(device,values=None):
            if values:raise PermissionError('control refused')
            return [self.metadata('brightness')]
        with patch.object(camera,'controls',controls):
            self.assertIn('control refused',camera.restore_controls('/dev/test',{'brightness':'128'})[0])
    def test_manual_control_is_restored_after_auto_mode_is_disabled(self):
        state={'auto':True};writes=[]
        def controls(device,values=None):
            if values:
                writes.append(values)
                if values.get('auto_exposure')=='manual_mode':state['auto']=False
            return [self.metadata('auto_exposure'),self.metadata('exposure_time_absolute',state['auto'])]
        with patch.object(camera,'controls',controls):
            camera.restore_controls('/dev/test',{'auto_exposure':'manual_mode','exposure_time_absolute':'200'})
        self.assertEqual(writes[-1],{'exposure_time_absolute':'200'})

    def test_live_preset_never_reopens_capture_and_save_reads_fresh_values(self):
        worker=camera.Worker();events=[];state={'brightness':'128'}
        worker.device='/dev/test';worker.state='Live';worker.pipeline=object();worker.config={'controls':{}}
        worker.event_sink=events.append
        def controls(device,values=None):
            if values:state.update(values)
            return [self.metadata('brightness')|{'value':state['brightness']}]
        try:
            with patch.object(camera,'controls',controls),patch.object(camera,'devices',return_value=[]),patch.object(camera,'output_device',return_value=''),patch.object(worker,'start') as start,patch.object(worker,'stop') as stop:
                worker.handle({'op':'apply_preset','values':{'brightness':'160'},'request':'apply_camera_preset'})
                self.assertEqual(worker.state,'Live')
                start.assert_not_called();stop.assert_not_called()
                state['brightness']='175'
                worker.handle({'op':'read_controls','request':'save_camera'})
                self.assertEqual(events[-1]['request'],'save_camera')
                self.assertEqual(events[-1]['controls'][0]['value'],'175')
                with patch.object(camera,'controls',side_effect=PermissionError('Camera removed')):
                    worker.handle({'op':'read_controls','request':'save_camera'})
                self.assertEqual(events[-1]['controls_error'],'Camera removed')
        finally:worker.asset_dir.cleanup()

    def test_auto_controls_requiring_reopen_are_not_restored(self):
        metadata=self.metadata('auto_vendor_mode')|{'reopener':True}
        with patch.object(camera,'controls',return_value=[metadata]) as controls:
            camera.restore_controls('/dev/test',{'auto_vendor_mode':'1'})
        self.assertFalse(any(len(call.args)>1 for call in controls.call_args_list))

class AssetsTest(unittest.TestCase):
    def test_svg_text_and_cache(self):
        worker=camera.Worker()
        try:
            with tempfile.TemporaryDirectory() as directory:
                svg=Path(directory)/'logo.svg'
                svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 80 40"><rect width="80" height="40" fill="red"/></svg>')
                image=worker.asset({'path':str(svg)},1280)
                self.assertEqual((image['width'],image['height']),(80,40))
                text={'kind':'Text','text':'Hello · camera','font':'Sans','font_size':32,'width':.5,'color':[255]*4}
                first=worker.asset(text,1280)
                self.assertIs(first,worker.asset(text,1280))
                import subprocess
                pixels=subprocess.run([camera.raster_binary(),'--tray-icon',first['path']],capture_output=True,check=True).stdout
                self.assertEqual(len(pixels),4096)
                self.assertTrue(any(pixels))
                text['text']='Another scene'
                self.assertNotEqual(first['key'],worker.asset(text,1280)['key'])
        finally:worker.asset_dir.cleanup()

if __name__=='__main__':unittest.main()
