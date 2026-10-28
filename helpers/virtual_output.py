"""A persistent V4L2 writer. Standby needs no camera or graphics pipeline."""
import os
import threading
from fcntl import ioctl
import cameractrls as cc
from security import capture_size

class VirtualOutput:
    def __init__(self, device):
        self.device = device
        self.fd = None
        self.size = None
        self.blank = b''
        self.lock = threading.Lock()

    @property
    def enabled(self):
        return self.fd is not None

    def enable(self, width, height, fps):
        with self.lock:
            size = capture_size({'width': width, 'height': height, 'fps': fps})
            if self.fd is not None:
                if size != self.size:
                    raise RuntimeError('Disable virtual output before changing resolution or frame rate')
                return
            device = self.device()
            if not device:
                raise RuntimeError('Laptop Control Camera is missing; run the system installer')
            fd = os.open(device, os.O_RDWR | os.O_NONBLOCK)
            try:
                fmt = cc.v4l2_format()
                fmt.type = 2  # V4L2_BUF_TYPE_VIDEO_OUTPUT
                fmt.fmt.pix.width, fmt.fmt.pix.height = size[:2]
                fmt.fmt.pix.pixelformat = cc.V4L2_PIX_FMT_YUYV
                fmt.fmt.pix.field = 1  # V4L2_FIELD_NONE
                fmt.fmt.pix.bytesperline = size[0] * 2
                fmt.fmt.pix.sizeimage = size[0] * size[1] * 2
                ioctl(fd, cc.VIDIOC_S_FMT, fmt)
                parm = cc.v4l2_streamparm()
                parm.type = 2
                parm.parm.output.timeperframe.numerator = 1
                parm.parm.output.timeperframe.denominator = size[2]
                ioctl(fd, cc.VIDIOC_S_PARM, parm)
                if (fmt.fmt.pix.width, fmt.fmt.pix.height) != size[:2]:
                    raise RuntimeError('The virtual camera could not accept the requested size')
                self.blank = bytes((16,128,16,128)) * (size[0]*size[1]//2)
                os.write(fd, self.blank)
                self.fd, self.size = fd, size
            except Exception:
                os.close(fd)
                raise

    def write(self, data):
        with self.lock:
            if self.fd is not None:
                try:
                    os.write(self.fd, data)
                except BlockingIOError:
                    pass  # A slow consumer must not delay the source or preview.

    def standby(self):
        self.write(self.blank)

    def close(self):
        with self.lock:
            if self.fd is not None:
                os.close(self.fd)
            self.fd = None
            self.size = None
            self.blank = b''
