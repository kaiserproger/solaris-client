"""Analysis-only verified-archive rig export. Never imported/invoked by native runtime.
All originals, javap transcripts and derived data live under the explicit ignored output.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import zipfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if '.analysis' not in args.output.parts:
        raise ValueError('Mojang-derived output must remain in ignored .analysis')
    args.output.mkdir(parents=True, exist_ok=True)
    libraries = args.output / 'libraries'
    libraries.mkdir(exist_ok=True)
    with zipfile.ZipFile(args.server) as server:
        for entry in server.infolist():
            if entry.filename.startswith('META-INF/libraries/') and entry.filename.endswith('.jar'):
                if entry.file_size > 64 * 1024 * 1024:
                    raise ValueError('dependency byte bound')
                (libraries / Path(entry.filename).name).write_bytes(server.read(entry))
    digest = hashlib.sha256(args.client.read_bytes()).hexdigest()
    classpath = str(args.client) + ':' + str(libraries / '*')
    subprocess.run(['javac', '-cp', classpath, '-d', str(args.output), str(Path(__file__).with_name('ExportModels.java'))], check=True)
    subprocess.run(['java', '-cp', str(args.output) + ':' + classpath, 'ExportModels', str(args.output / 'models.json'), digest], check=True)
    document = json.loads((args.output / 'models.json').read_text())
    with zipfile.ZipFile(args.client) as client:
        renderers = [n[:-6].replace('/', '.') for n in client.namelist()
                     if n.startswith('net/minecraft/client/renderer/entity/') and n.endswith('Renderer.class') and '$' not in n]
        transcript = subprocess.check_output(['javap', '-c', '-p', '-classpath', str(args.client), *renderers], text=True)
        (args.output / 'renderers.javap').write_text(transcript)
        bodies = {}
        parents = {}
        for body in re.split(r'(?=Compiled from ")', transcript):
            header = body.split('{', 1)[0]
            while '<' in header:
                stripped = re.sub(r'<[^<>]*>', '', header)
                if stripped == header:
                    raise ValueError('renderer generic signature cannot be balanced')
                header = stripped
            match = re.search(r'\bclass ([\w.]+)', header)
            parent = re.search(r'\bextends ([\w.]+)', header)
            if match:
                bodies[match[1]] = body
                parents[match[1]] = parent[1] if parent else None
        families = {}
        for name, schema in document['entities'].items():
            entity_class = schema['class'].rsplit('.', 1)[-1]
            defaults = document['renderer_defaults'].get(name, {})
            candidates = [defaults['renderer_class']] if 'renderer_class' in defaults else [
                k for k in bodies if k.rsplit('.', 1)[-1].lower() == (entity_class + 'Renderer').lower()]
            texture_candidates = []
            for candidate in candidates:
                seen = set()
                while candidate in bodies and candidate not in seen:
                    seen.add(candidate)
                    texture_candidates.extend(re.findall(r'// String (textures/entity/[^\s%]+\.png)', bodies[candidate]))
                    candidate = parents.get(candidate)
            texture_candidates = list(dict.fromkeys(texture_candidates))
            def texture_literals(value):
                if isinstance(value, str):
                    if ':textures/' in value or value.startswith('textures/'):
                        yield value
                elif isinstance(value, dict):
                    for child in value.values():
                        yield from texture_literals(child)
                elif isinstance(value, list):
                    for child in value:
                        yield from texture_literals(child)
            texture_candidates.extend(texture_literals(defaults.get('materials', {})))
            texture_candidates = list(dict.fromkeys(t.removeprefix('minecraft:') for t in texture_candidates))
            root = defaults.get('rig', name + '#main')
            if root not in document['roots']:
                continue
            short = name.split(':')[1]
            exact = [t for t in texture_candidates if t.rsplit('/', 1)[-1] == short + '.png']
            texture = defaults.get('texture')
            if texture == 'minecraft:missingno':
                texture = None
            texture = texture or (exact[0] if exact else (texture_candidates[0] if len(texture_candidates) == 1 else None))
            # Ambiguous/default materials remain explicit, never guessed from directory proximity.
            families[name] = {'rig': root, 'texture': texture,
                              'baby_rig': name + '_baby#main' if name + '_baby#main' in document['roots'] else None,
                              'textures': texture_candidates,
                              'renderer_class': defaults.get('renderer_class', candidates[0] if candidates else None),
                              'materials': defaults.get('materials', {})}
        # These aliases are registry identities sharing the actual renderer rig/material family.
        aliases = {'cave_spider': 'spider', 'spectral_arrow': 'arrow', 'glow_squid': 'squid'}
        for alias, base in aliases.items():
            key, basekey = 'minecraft:' + alias, 'minecraft:' + base
            if basekey in families:
                if key not in families:
                    entry = dict(families[basekey])
                    render = next((v for k,v in bodies.items() if k.endswith('.' + ''.join(p.title() for p in alias.split('_')) + 'Renderer')), '')
                    textures = re.findall(r'// String (textures/entity/[^\s%]+\.png)', render)
                    if len(textures) == 1:
                        entry['texture'] = textures[0]
                    families[key] = entry
        document['families'] = families
        # Server Configuration decides numeric IDs; these named originals supply omitted built-in NBT only.
        registries = {}
        for entry in client.infolist():
            pieces = entry.filename.split('/')
            if len(pieces) == 4 and pieces[:2] == ['data', 'minecraft'] and pieces[2].endswith('_variant') and pieces[3].endswith('.json'):
                registries.setdefault('minecraft:' + pieces[2], {})['minecraft:' + pieces[3][:-5]] = json.loads(client.read(entry))
        document['builtin_variants'] = registries
    data = json.dumps(document, separators=(',', ':')).encode()
    (args.output / 'models.json').write_bytes(data)
    receipt = {'schema': 1, 'game_version': '26.1.2', 'client_sha256': digest,
               'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
               'models_sha256': hashlib.sha256(data).hexdigest(), 'roots': len(document['roots']),
               'entity_types': len(document['entities']), 'nonfinite_polygons': document['nonfinite_polygons'],
               'tool_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
               'exporter_sha256': hashlib.sha256(Path(__file__).with_name('ExportModels.java').read_bytes()).hexdigest(),
               'distribution': 'local Mojang-derived cache; forbidden in source/releases/public CI'}
    (args.output / 'provenance.json').write_text(json.dumps(receipt, indent=2))
    print(json.dumps(receipt))


if __name__ == '__main__':
    main()
