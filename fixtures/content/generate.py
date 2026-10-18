#!/usr/bin/env python3
"""Generate original fixture media; no Minecraft inputs or external assets."""
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import wave
import zlib

ROOT = Path(__file__).resolve().parent


def png(path, alpha):
    width = height = 16
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            bright = ((x // 4) ^ (y // 4)) & 1
            rows.extend((240 if bright else 24, 176 if bright else 64,
                         48 if bright else 208, 0 if alpha and (x + y) % 4 == 0 else 255))
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(bytes(rows), 9)) + chunk(b'IEND', b''))


def main():
    assets = ROOT / 'assets' / 'solaris_fixture'
    png(assets / 'textures' / 'block' / 'checker.png', False)
    png(assets / 'textures' / 'block' / 'cutout.png', True)
    sound = assets / 'sounds' / 'confirmation.wav'
    sound.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(sound), 'wb') as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(48000)
        frames = bytearray()
        for i in range(12000):
            t = i / 48000
            envelope = min(1.0, i / 240) * max(0.0, (12000 - i) / 2400)
            amplitude = 0.22 * min(1.0, envelope) * (math.sin(2 * math.pi * 440 * t) + 0.3 * math.sin(2 * math.pi * 660 * t))
            frames.extend(struct.pack('<h', round(amplitude * 32767)))
        out.writeframes(frames)
    subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
                    '-i', str(sound), '-map_metadata', '-1', '-c:a', 'libvorbis',
                    '-q:a', '4', '-fflags', '+bitexact', '-flags:a', '+bitexact',
                    str(sound.with_suffix('.ogg'))], check=True)
    (assets / 'sounds.json').write_text(json.dumps({
        'confirmation': {'sounds': ['solaris_fixture:confirmation']}
    }, indent=2) + '\n')
    (ROOT / 'pack.mcmeta').write_text(json.dumps({
        'pack': {'description': 'Solaris original redistributable fixtures',
                 'min_format': [84, 0], 'max_format': [84, 0]}
    }, indent=2) + '\n')
    model = {'textures': {'all': 'solaris_fixture:block/checker'}, 'elements': [
        {'from': [0, 0, 0], 'to': [16, 16, 16], 'faces': {
            face: {'uv': [0, 0, 16, 16], 'texture': '#all', 'cullface': face}
            for face in ('down', 'up', 'north', 'south', 'west', 'east')}}]}
    model_path = assets / 'models' / 'block' / 'checker.json'
    model_path.parent.mkdir(parents=True, exist_ok=True)
    model_path.write_text(json.dumps(model, indent=2) + '\n')
    files = []
    for path in sorted([ROOT / 'pack.mcmeta', *assets.rglob('*')]):
        if path.is_file():
            data = path.read_bytes()
            files.append({'path': str(path.relative_to(ROOT)), 'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()})
    (ROOT / 'provenance.json').write_text(json.dumps({
        'schema': 1, 'namespace': 'solaris_fixture', 'license': 'MIT OR Apache-2.0',
        'copyright': '2026 Solaris Client contributors',
        'origin': 'Original procedural checker/cutout image, authored cuboid model and mathematical synthesized tone; no Mojang input bytes.',
        'generator': 'generate.py', 'texture': {'width': 16, 'height': 16, 'format': 'RGBA8 PNG'},
        'sound': {'formats': ['PCM16 WAV', 'Vorbis OGG'], 'sample_rate': 48000, 'channels': 1, 'samples': 12000},
        'ogg_encoder': 'ffmpeg libvorbis q4 bitexact; byte identity requires the same encoder build',
        'files': files}, indent=2) + '\n')


if __name__ == '__main__':
    main()
