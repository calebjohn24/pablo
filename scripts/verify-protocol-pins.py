"""Offline K01 identities. Reads named source/SDK files only; never loads env files.

No update mode: changing a pin requires reviewing and editing the manifest.
--sdk-report is the subprocess interface for installed independent Python peers.
"""
import hashlib
import importlib.metadata
import json
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'docs/protocol-pins.json'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def sdk_report(names):
    result = {}
    for name in names:
        dist = importlib.metadata.distribution(name)
        files = sorted(p for p in dist.files if str(p).endswith('.py') or '/schemas/' in str(p))
        assert files, f'No source files in {name}'
        state = hashlib.sha256()
        for file in files:
            state.update(str(file).encode())
            state.update(b'\0')
            state.update(dist.locate_file(file).read_bytes())
            state.update(b'\0')
        result[name] = {'version': dist.version, 'source_files': len(files), 'source_sha256': state.hexdigest()}
    return result


def verify():
    manifest = json.loads(MANIFEST.read_text())
    assert manifest['schema_version'] == 1
    for path, expected in manifest['files'].items():
        assert not Path(path).is_absolute() and '..' not in Path(path).parts
        data = (ROOT / path).read_bytes()
        assert {'bytes': len(data), 'sha256': digest(data)} == expected, f'File identity drift: {path}'
    cargo = tomllib.loads((ROOT / 'Cargo.lock').read_text())['package']
    for expected in manifest['rust_sdks']:
        matches = [p for p in cargo if p['name'] == expected['name'] and p['version'] == expected['version']]
        assert len(matches) == 1 and all(matches[0].get(k) == v for k, v in expected.items()), f'Cargo SDK drift: {expected["name"]}'
    npm = json.loads((ROOT / 'package-lock.json').read_text())['packages']
    for name, expected in manifest['node_sdks'].items():
        assert all(npm['node_modules/' + name].get(k) == v for k, v in expected.items()), f'npm SDK drift: {name}'
        installed = json.loads((ROOT / 'node_modules' / name / 'package.json').read_text())
        assert installed['version'] == expected['version'], f'Installed npm SDK drift: {name}'
    for peer in manifest['python_peers']:
        actual = json.loads(subprocess.check_output([str(ROOT / peer['interpreter']), str(Path(__file__).resolve()), '--sdk-report', *peer['packages']], cwd=ROOT))
        assert actual == peer['packages'], f'Installed Python SDK drift: {peer["interpreter"]}'
    matrix = json.loads((ROOT / 'docs/protocol-compatibility.json').read_text())
    assert {p['protocol'] for p in matrix['operations']} == set(manifest['protocols'])
    for row in matrix['operations']:
        assert row['gates'] and (ROOT / row['identity_or_contract']).is_file()
        for gate in row['gates']:
            for token in gate.split():
                if token.endswith(('.ts', '.mjs', '.py')):
                    assert (ROOT / token).is_file(), f'Missing gate: {token}'
    registry = json.loads((ROOT / 'docs/acp-extensions.json').read_text())
    for extension in registry['extensions']:
        assert extension['uri'].startswith('https://github.com/calebjohn24/pablo/')
        assert extension['privacy'] and extension['generic_peer_fallback'] and extension['bounds']
        for key in ['schema', 'notification_schema']:
            if key in extension:
                path, _, pointer = extension[key].partition('#')
                value = json.loads((ROOT / path).read_text())
                for part in pointer.split('/')[1:]:
                    value = value[part.replace('~1', '/').replace('~0', '~')]
    print(f'K01 immutable identities passed: {len(manifest["files"])} files, {len(manifest["rust_sdks"])} Rust SDKs, {len(manifest["node_sdks"])} Node SDKs, {len(manifest["python_peers"])} independent Python peers; seven protocol surfaces.')


if __name__ == '__main__':
    if sys.argv[1:2] == ['--sdk-report']:
        print(json.dumps(sdk_report(sys.argv[2:]), sort_keys=True))
    else:
        assert len(sys.argv) == 1, 'Usage: python3 scripts/verify-protocol-pins.py'
        verify()
