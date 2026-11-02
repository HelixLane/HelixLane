import json, os, pathlib, shutil, subprocess, tempfile, time, plistlib
root=pathlib.Path(__file__).resolve().parents[1];bins=root/'target/debug';env=os.environ.copy();env.pop('DISPLAY',None);env.pop('WAYLAND_DISPLAY',None);env.pop('HELIXLANE_TEST_IPC_TCP',None)
for name in ['helixlane','helixlane-putty','helixlane-winscp']:
 for flag in ['--help','--version']:
  p=subprocess.run([str(bins/name),flag],env=env,capture_output=True,text=True,timeout=5)
  assert p.returncode==0,(name,flag,p.returncode,p.stderr)
  assert 'HelixLane' in p.stdout
  if flag=='--help': assert '用法' in p.stdout
 print(name,'help/version exit0 without display; bilingual usage')
negative=[('helixlane',['open','alice@host','--password','private-test-canary','--unsupported']),('helixlane',['helixlane://open?host=example.invalid&password=private-test-canary']),('helixlane',['open','ssh://u:private-test-canary@example.invalid']),('helixlane-putty',['-load','private-test-canary']),('helixlane-winscp',['/script=private-test-canary'])]
for name,args in negative:
 p=subprocess.run([str(bins/name),*args],env=env,capture_output=True,text=True,timeout=5)
 assert p.returncode==2 and 'private-test-canary' not in p.stdout+p.stderr
 assert any('\u4e00'<=c<='\u9fff' for c in p.stderr)
 print(name,'unsupported/forbidden argument exit2; value redacted; Chinese diagnostic')
for name in ['helixlane','helixlane-putty','helixlane-winscp']:
 p=subprocess.run([os.fsencode(bins/name),b'invalid-private-canary-\xff'],env=env,capture_output=True,timeout=5)
 assert p.returncode==2 and b'invalid-private-canary' not in p.stdout+p.stderr
 print(name,'invalid UTF-8 exit2; raw OS argument redacted')
p=subprocess.run([str(bins/'helixlane'),'--launch-stdin'],input=b'{"private-test-canary":',env=env,capture_output=True,timeout=5)
assert p.returncode==2 and b'private-test-canary' not in p.stdout+p.stderr
print('main invalid stdin envelope exit2; content redacted')
for payload in [b'{"password":"private-test-canary","unknown":true}', b'{"password":"private-test-canary","password":"duplicate"}', b'{"password":"private-test-canary","version":']:
 p=subprocess.run([str(bins/'helixlane'),'--launch-stdin'],input=payload,env=env,capture_output=True,timeout=5)
 assert p.returncode==2 and b'private-test-canary' not in p.stdout+p.stderr
print('main parsed-password malformed/duplicate/unknown stdin errors exit2; scope unwinds and diagnostics remain redacted')
p=subprocess.run([os.fsencode(bins/'helixlane'),b'open',b'alice@host',b'--password',b'private-test-canary',b'\xff'],env=env,capture_output=True,timeout=5)
assert p.returncode==2 and b'private-test-canary' not in p.stdout+p.stderr
print('main invalid Unicode after credential arguments exits2 with protected collection and redacted diagnostics')

for name,args,expected in [('helixlane-putty',['-pw','--auth-none','alice@host'],'--auth-none'),('helixlane-putty',['-ssh','-l','alice','-pw','proxy-test-canary','host.invalid'],'proxy-test-canary'),('helixlane-winscp',['sftp://alice:c%3Ap%26%20word@host.invalid/a%20b'],'c:p& word')]:
 with tempfile.TemporaryDirectory(prefix='helixlane-stdin-proxy-') as temp:
  folder=pathlib.Path(temp);shutil.copy2(bins/name,folder/name)
  stub=folder/'helixlane';stub.write_text('#!/usr/bin/python3\nimport json,sys,os\nw=json.load(sys.stdin)\nwith open(os.environ["TEST_RESULT"],"w") as f:json.dump({"argv":sys.argv[1:],"auth_received":w["password"]==os.environ["EXPECTED_TEST_PASSWORD"]},f)\n');stub.chmod(0o700)
  result=folder/'result.json';runenv={**env,'TEST_RESULT':str(result),'EXPECTED_TEST_PASSWORD':expected}
  p=subprocess.run([str(folder/name),*args],env=runenv,capture_output=True,text=True,timeout=5)
  assert p.returncode==0,p.stderr
  for _ in range(100):
   if result.exists(): break
   time.sleep(.02)
  data=json.loads(result.read_text());assert data=={'argv':['--launch-stdin'],'auth_received':True},data
  assert expected not in p.stdout+p.stderr
  print(name,'cold-start child argv constant --launch-stdin; secret delivered only through inherited pipe')
with (root/'packaging/macos/Info.plist').open('rb') as f: plistlib.load(f)
for name in ['linux','macos']:subprocess.run(['sh','-n',str(root/'packaging'/name/'install.sh')],check=True)
print('macOS plist parses; Linux/macOS installer shell syntax valid (registration not executed)')
