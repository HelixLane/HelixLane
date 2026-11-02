#!/usr/bin/env python3
"""Real executable CLI contracts on Linux/macOS/Windows, with redirected streams."""
import os, pathlib, struct, subprocess
root=pathlib.Path(__file__).resolve().parents[1];suffix='.exe' if os.name=='nt' else '';bins=root/os.environ.get('HELIXLANE_BIN_DIR','target/debug')
for name in ['helixlane','helixlane-putty','helixlane-winscp']:
    binary=bins/(name+suffix)
    for flag in ['--help','--version']:
        p=subprocess.run([str(binary),flag],capture_output=True,timeout=10)
        assert p.returncode==0 and b'HelixLane' in p.stdout,(name,flag,p.returncode,p.stderr)
    p=subprocess.run([str(binary),'--unsupported-private-canary'],capture_output=True,timeout=10)
    assert p.returncode==2 and b'private-canary' not in p.stdout+p.stderr,(name,p.returncode)
    if os.name=='nt':
        image=binary.read_bytes();pe=struct.unpack_from('<I',image,0x3c)[0]
        assert image[pe:pe+4]==b'PE\0\0'
        assert struct.unpack_from('<H',image,pe+24+68)[0]==2, 'GUI subsystem required: no Explorer console allocation'
    print(f'PASS {name}: real help/version streams, unsupported argument exit2, redaction'+('; PE GUI subsystem' if os.name=='nt' else ''))
p=subprocess.run([str(bins/('helixlane'+suffix)),'--launch-stdin'],input=b'{"password":"private-canary","unknown":true}',capture_output=True,timeout=10)
assert p.returncode==2 and b'private-canary' not in p.stdout+p.stderr
print('PASS main stdin malformed-envelope redaction/exit2')
