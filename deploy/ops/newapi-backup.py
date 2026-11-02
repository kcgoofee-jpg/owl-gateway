#!/usr/bin/env python3
"""Private, consistent local backups of New API and its OWL account bridge."""
import fcntl, gzip, os, pathlib, shutil, sqlite3, tarfile, tempfile, time

os.umask(0o077)
root=pathlib.Path('/opt/newapi/backups')
root.mkdir(parents=True,exist_ok=True)
root.chmod(0o700)
lock=open(root/'.backup.lock','a')
fcntl.flock(lock,fcntl.LOCK_EX)
stamp=time.strftime('%Y-%m-%d-%H%M%S')
work=pathlib.Path(tempfile.mkdtemp(prefix='.account-backup-',dir=root))
try:
    for source,prefix in [('/opt/newapi/data/one-api.db','one-api'),('/opt/service-tools/data/nai_gate.db','owl-accounts')]:
        snapshot=work/(prefix+'.db')
        src=sqlite3.connect('file:'+source+'?mode=ro',uri=True)
        dst=sqlite3.connect(snapshot)
        src.backup(dst)
        assert dst.execute('PRAGMA quick_check').fetchone()[0]=='ok'
        dst.close();src.close()
        packed=work/(prefix+'.db.gz')
        with open(snapshot,'rb') as a,gzip.open(packed,'wb') as b:shutil.copyfileobj(a,b)
        os.replace(packed,root/(prefix+'-'+stamp+'.db.gz'))
    config=work/'welfare-config.tar.gz'
    with tarfile.open(config,'w:gz') as archive:
        for name in ['opt/welfare/ai.secret','opt/welfare/ai_account.py','opt/newapi/start.sh','opt/newapi/credentials.txt',
                     'opt/service-tools/.env','opt/owl/ops/ai-panel.js','opt/owl/ops/theme-init.js',
                     'opt/owl/ops/ai-panel-heal.sh','etc/systemd/system/owl-ai-account.service']:
            if pathlib.Path('/'+name).is_file():archive.add('/'+name,arcname=name,recursive=False)
    os.replace(config,root/('welfare-config-'+stamp+'.tar.gz'))
    for path in root.iterdir():
        if path.is_file() and path.name.startswith(('one-api-','owl-accounts-','welfare-config-','welfare-')):
            path.chmod(0o600)
            if path.stat().st_mtime<time.time()-14*86400:path.unlink()
    print('account backup verified:',stamp,'two SQLite databases and private bridge configuration')
finally:
    shutil.rmtree(work)
