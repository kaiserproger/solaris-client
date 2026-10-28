"""Keep a keyboard capability on an isolated wlroots seat, without sending keys.

A headless backend has no input devices, hence no wl_keyboard.enter and no
native-window focus. The real application must receive compositor focus before
its MCP can control it. This uses the standard virtual-keyboard protocol only
on the private socket passed by pacing_headless, never uinput or the host seat.
"""
import array
import ctypes
import os
import socket
import struct


def _string(value):
    data = value.encode() + b'\0'
    return struct.pack('I', len(data)) + data + b'\0' * (-len(data) % 4)


class KeyboardSeat:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.settimeout(20)
        self.socket.connect(str(path))
        self.pending = bytearray()
        self._send(1, 1, struct.pack('I', 2))  # wl_display.get_registry
        self._send(1, 0, struct.pack('I', 3))  # wl_display.sync
        globals_ = {}
        while True:
            obj, opcode, body = self._event()
            if obj == 3:
                break
            if obj == 2 and opcode == 0:
                name, size = struct.unpack_from('II', body)
                interface = body[8:8 + size - 1].decode()
                globals_[interface] = name
            if obj == 1 and opcode == 0:
                raise RuntimeError(f'Wayland registry error: {body!r}')
        for interface, target in [('wl_seat', 4), ('zwp_virtual_keyboard_manager_v1', 5)]:
            self._send(2, 0, struct.pack('I', globals_[interface]) + _string(interface) + struct.pack('II', 1, target))
        self._send(5, 0, struct.pack('II', 4, 6))  # create_virtual_keyboard(seat, id)
        xkb = ctypes.CDLL('libxkbcommon.so.0')
        xkb.xkb_context_new.argtypes, xkb.xkb_context_new.restype = [ctypes.c_int], ctypes.c_void_p
        xkb.xkb_keymap_new_from_names.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int]
        xkb.xkb_keymap_new_from_names.restype = ctypes.c_void_p
        xkb.xkb_keymap_get_as_string.argtypes = [ctypes.c_void_p, ctypes.c_int]
        xkb.xkb_keymap_get_as_string.restype = ctypes.c_void_p
        xkb.xkb_keymap_unref.argtypes = [ctypes.c_void_p]
        xkb.xkb_context_unref.argtypes = [ctypes.c_void_p]
        context = xkb.xkb_context_new(0)
        keymap = xkb.xkb_keymap_new_from_names(context, None, 0)
        text = xkb.xkb_keymap_get_as_string(keymap, 1)
        if not text:
            raise RuntimeError('Unable to construct private keyboard keymap')
        keymap_bytes = ctypes.string_at(text) + b'\0'
        libc = ctypes.CDLL(None)
        libc.free.argtypes = [ctypes.c_void_p]
        libc.free(text)
        xkb.xkb_keymap_unref(keymap)
        xkb.xkb_context_unref(context)
        fd = os.memfd_create('solaris-private-keymap', os.MFD_CLOEXEC)
        try:
            os.write(fd, keymap_bytes)
            payload = struct.pack('IIII', 6, (16 << 16), 1, len(keymap_bytes))
            sent = self.socket.sendmsg([payload], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
            if sent != len(payload):
                raise RuntimeError('Short private keymap write')
        finally:
            os.close(fd)
        self._send(1, 0, struct.pack('I', 7))
        while True:
            obj, opcode, body = self._event()
            if obj == 7:
                break
            if obj == 1 and opcode == 0:
                raise RuntimeError(f'Wayland keymap error: {body!r}')

    def _send(self, obj, opcode, body):
        self.socket.sendall(struct.pack('II', obj, ((len(body) + 8) << 16) | opcode) + body)

    def _event(self):
        while len(self.pending) < 8:
            self._receive()
        obj, word = struct.unpack_from('II', self.pending)
        size, opcode = word >> 16, word & 0xffff
        if size < 8:
            raise RuntimeError('Invalid Wayland event size')
        while len(self.pending) < size:
            self._receive()
        body = bytes(self.pending[8:size])
        del self.pending[:size]
        return obj, opcode, body

    def _receive(self):
        data = self.socket.recv(65536)
        if not data:
            raise RuntimeError('Private Wayland connection closed')
        self.pending.extend(data)

    def close(self):
        self.socket.close()
