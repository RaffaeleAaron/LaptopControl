#!/usr/bin/env python3
"""Root-only regression fixture. Touches only a temporary tree, never system setup."""
import os
from pathlib import Path
import pwd
import shutil
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
if os.geteuid()!=0:raise SystemExit('Run this isolated privilege-boundary fixture as root')
account=pwd.getpwnam('nobody')
with tempfile.TemporaryDirectory(prefix='laptop-control-boundary-') as d:
    p=Path(d);p.chmod(0o755)
    home=p/'home';home.mkdir();os.chown(home,account.pw_uid,account.pw_gid)
    templates=p/'templates';templates.mkdir()
    for name in ('laptop-control-autostart.desktop','laptop-control.service'):
        shutil.copyfile(ROOT/'packaging'/name,templates/name)
    helper=p/'user-setup';shutil.copyfile(ROOT/'packaging/laptop-control-user-setup',helper);helper.chmod(0o755)
    victim=p/'root-owned';victim.write_text('must remain unchanged');victim.chmod(0o600)
    config=home/'.config/systemd/user';config.mkdir(parents=True)
    for path in (home/'.config',home/'.config/systemd',config):os.chown(path,account.pw_uid,account.pw_gid)
    (config/'laptop-control.service').symlink_to(victim)
    env=['env',f'HOME={home}',f'XDG_CONFIG_HOME={home/".config"}',f'XDG_DATA_HOME={home/".local/share"}',f'XDG_RUNTIME_DIR={p/"no-session"}']
    result=subprocess.run(['runuser','-u',account.pw_name,'--',*env,str(helper),str(templates)],capture_output=True,text=True)
    assert result.returncode==0,result.stderr
    assert victim.read_text()=='must remain unchanged'
    assert not (config/'laptop-control.service').is_symlink()
    # A malicious parent symlink cannot turn a privileged target into a user write.
    protected=p/'protected';protected.mkdir(mode=0o700)
    target=protected/'laptop-control.desktop';target.write_text('protected')
    autostart=home/'.config/autostart'
    shutil.rmtree(autostart);autostart.symlink_to(protected,target_is_directory=True)
    result=subprocess.run(['runuser','-u',account.pw_name,'--',*env,str(helper),str(templates)],capture_output=True,text=True)
    assert result.returncode!=0
    assert target.read_text()=='protected'
    print('User integration cannot overwrite root-owned symlink targets: PASS')
