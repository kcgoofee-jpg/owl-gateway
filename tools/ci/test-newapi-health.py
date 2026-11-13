"""Test only the read-only projection, without importing deployment credentials."""
import ast,json,sqlite3,tempfile,time,unittest
from pathlib import Path
source=Path(__file__).resolve().parents[2]/'deploy/ops/newapi-account.py'
node=next(n for n in ast.parse(source.read_text()).body if isinstance(n,ast.FunctionDef) and n.name=='upstream_health')
class HealthTest(unittest.TestCase):
    def test_passive_health_distinguishes_opened_success_error_and_unknown(self):
        with tempfile.TemporaryDirectory() as d:
            path=str(Path(d)/'newapi.db');c=sqlite3.connect(path)
            c.executescript('CREATE TABLE channels(models TEXT,"group" TEXT,status INTEGER);CREATE TABLE logs(id INTEGER PRIMARY KEY,model_name TEXT,type INTEGER,created_at INTEGER,use_time REAL,content TEXT);')
            c.execute('INSERT INTO channels VALUES(?,?,1)',('synthetic-slow,synthetic-unknown,synthetic-failed','default'))
            now=int(time.time());c.execute('INSERT INTO logs VALUES(1,?,2,?,72,NULL)',('synthetic-slow',now));c.execute('INSERT INTO logs VALUES(2,?,5,?,1,?)',('synthetic-failed',now,'401 bad credential synthetic-private-value'));c.execute('ALTER TABLE channels ADD COLUMN id INTEGER DEFAULT 1');c.execute('ALTER TABLE channels ADD COLUMN test_time INTEGER DEFAULT 0');c.execute('ALTER TABLE channels ADD COLUMN response_time INTEGER DEFAULT 120');c.commit();c.close()
            ns={'sqlite3':sqlite3,'time':time,'NEWAPI_DB':path};exec(compile(ast.Module(body=[node],type_ignores=[]),str(source),'exec'),ns)
            result=ns['upstream_health']();models=result['models']
            self.assertEqual(models['synthetic-slow']['state'],'recent_success');self.assertEqual(models['synthetic-slow']['average_seconds_60m'],72)
            self.assertEqual(models['synthetic-unknown']['state'],'unknown');self.assertEqual(models['synthetic-failed']['last_error_category'],'authentication')
            self.assertNotIn('synthetic-private-value',json.dumps(result));self.assertTrue(result['complete']);self.assertNotIn('channels',result)
            admin=ns['upstream_health'](True);self.assertEqual(set(admin['channels'][0]),{'id','status','models','last_test_at','response_ms'})
if __name__=='__main__': unittest.main()
