#!/usr/bin/env python3
"""Deterministic zip assembly from already-built, locked release binaries.
Fixed timestamps/order/modes; manifest hashes every payload file. Native linking,
SDK and compiler versions still affect binary bytes; CI records those versions.
"""
import argparse, hashlib, pathlib, stat, zipfile


def archive_name(path, root):
    """Use ZIP's portable separator before manifest hashing or archive writing."""
    return path.relative_to(root).as_posix()


def main():
    root=pathlib.Path(__file__).resolve().parents[1]
    p=argparse.ArgumentParser();p.add_argument('--platform',choices=['linux','macos','windows'],required=True);p.add_argument('--binaries',default='target/release');p.add_argument('--output',default='target/packages');a=p.parse_args()
    source=root/a.binaries;output=root/a.output;output.mkdir(parents=True,exist_ok=True)
    payload={};executable=set();suffix='.exe' if a.platform=='windows' else ''
    prefix='HelixLane.app/Contents/MacOS/' if a.platform=='macos' else 'bin/'
    for binary in ['helixlane','helixlane-putty','helixlane-winscp']:
        name=prefix+binary+suffix;payload[name]=(source/(binary+suffix)).read_bytes();executable.add(name)
    for name in ['README.md','README.zh-CN.md','LICENSE','Cargo.lock','vendor/russh-upstream-sha256.json','vendor/gpui-shell-upstream-sha256.json']:
        path=root/name
        if path.is_file():payload[name]=path.read_bytes()
    for vendor in ['russh','gpui-shell']:
        for path in sorted((root/'vendor'/vendor).glob('LICENSE*')):
            if path.is_file():payload[archive_name(path, root)]=path.read_bytes()
    for path in sorted((root/'docs').glob('*.md')):payload[archive_name(path, root)]=path.read_bytes()
    for path in sorted((root/'packaging'/a.platform).glob('*')):
        if path.is_file():
            name=archive_name(path, root);payload[name]=path.read_bytes()
            if path.suffix=='.sh':executable.add(name)
    if a.platform=='macos':payload['HelixLane.app/Contents/Info.plist']=(root/'packaging/macos/Info.plist').read_bytes()
    payload['SHA256SUMS']=(''.join(f'{hashlib.sha256(data).hexdigest()}  {name}\n' for name,data in sorted(payload.items()))).encode()
    archive=output/f'helixlane-0.1.0-{a.platform}.zip'
    with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=9) as z:
        for name,data in sorted(payload.items()):
            info=zipfile.ZipInfo(name,date_time=(2024,1,1,0,0,0));info.create_system=3;info.external_attr=(stat.S_IFREG | (0o755 if name in executable else 0o644))<<16;info.compress_type=zipfile.ZIP_DEFLATED;z.writestr(info,data)
    with zipfile.ZipFile(archive) as z:
        for line in z.read('SHA256SUMS').decode().splitlines():
            digest,name=line.split('  ',1);assert hashlib.sha256(z.read(name)).hexdigest()==digest
    print(f'PASS package manifest: {archive} ({len(payload)} files), sha256={hashlib.sha256(archive.read_bytes()).hexdigest()}')


if __name__ == "__main__":
    main()
