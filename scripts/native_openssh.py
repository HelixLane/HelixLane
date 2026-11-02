#!/usr/bin/env python3
"""Provision disposable full sshd fixtures. Linux CI only; requires passwordless sudo.
Never mutates the host sshd config. Explicit failure, never a successful skip.
"""
import json, os, pathlib, re, shutil, socket, subprocess, tempfile, time

def run(*args, **kw): return subprocess.run(args, check=True, **kw)
def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1',0)); return s.getsockname()[1]
root=pathlib.Path(tempfile.mkdtemp(prefix='helixlane-native-'));root.chmod(0o755)
user=f'hltest{os.getpid()}'
children=[]; agent=None; created=False
try:
    for binary in ['/usr/sbin/sshd','/usr/lib/openssh/sftp-server','/usr/bin/tmux','/usr/bin/ssh-agent']:
        assert pathlib.Path(binary).is_file(), f'Required native dependency missing: {binary}'
    run('sudo','-n','true')
    run('sudo','useradd','--create-home','--shell','/bin/bash',user);created=True
    password='helixlane-native-test-only'
    run('sudo','chpasswd',input=f'{user}:{password}\n',text=True)
    home=f'/home/{user}'; key=root/'client';host=root/'host'
    for path in [key,host]:run('ssh-keygen','-q','-t','ed25519','-N','','-f',str(path))
    run('sudo','install','-d','-o',user,'-g',user,'-m','700',f'{home}/.ssh')
    run('sudo','install','-o',user,'-g',user,'-m','600',str(key)+'.pub',f'{home}/.ssh/authorized_keys')
    run('sudo','mkdir','-p','/run/sshd')
    ports=[port(),port()]
    for index,p in enumerate(ports):
        config=root/f'sshd-{index}.conf';log=root/f'sshd-{index}.log'
        config.write_text(f'''Port {p}
ListenAddress 127.0.0.1
HostKey {host}
PidFile {root}/sshd-{index}.pid
AuthorizedKeysFile .ssh/authorized_keys
PasswordAuthentication yes
KbdInteractiveAuthentication yes
UsePAM yes
PubkeyAuthentication yes
PermitRootLogin no
AllowUsers {user}
AllowTcpForwarding yes
GatewayPorts clientspecified
Subsystem sftp /usr/lib/openssh/sftp-server
StrictModes yes
LogLevel VERBOSE
'''+('ForceCommand internal-sftp\n' if index else ''))
        run('sudo','/usr/sbin/sshd','-t','-f',str(config))
        children.append(subprocess.Popen(['sudo','/usr/sbin/sshd','-D','-e','-f',str(config)],stdout=log.open('w'),stderr=subprocess.STDOUT))
        for _ in range(100):
            assert children[-1].poll() is None, log.read_text()
            try:
                with socket.create_connection(('127.0.0.1',p),timeout=.1): break
            except OSError: time.sleep(.05)
        else: raise AssertionError('sshd listen timeout')
    agent_text=subprocess.check_output(['ssh-agent','-s'],text=True)
    env=os.environ.copy()
    for name in ['SSH_AUTH_SOCK','SSH_AGENT_PID']:
        env[name]=re.search(name+r'=([^;]+);',agent_text).group(1)
    agent=env
    run('ssh-add',str(key),env=env)
    fingerprint=subprocess.check_output(['ssh-keygen','-lf',str(host)+'.pub','-E','sha256'],text=True).split()[1]
    config=root/'client.json';config.write_text(json.dumps(dict(root=str(root),home=home,user=user,password=password,key=str(key),fingerprint=fingerprint,port=ports[0],sftp_port=ports[1])))
    env['HELIXLANE_NATIVE_CONFIG']=str(config)
    run('cargo','test','--locked','--features','shell-tests','--test','native_openssh','--','--ignored','--nocapture',env=env)
finally:
    for p in children:
        if p.poll() is None:
            subprocess.run(['sudo','pkill','-TERM','-P',str(p.pid)],check=False);p.wait(timeout=10)
    if agent:run('ssh-agent','-k',env=agent,stdout=subprocess.DEVNULL)
    if created:run('sudo','userdel','--remove',user)
    logs=pathlib.Path('target/native-logs');logs.mkdir(parents=True,exist_ok=True)
    for log in root.glob('*.log'):shutil.copy2(log,logs/log.name)
    shutil.rmtree(root)
