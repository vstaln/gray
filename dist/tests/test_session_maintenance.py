"""Large superseded history must not block transport resumption."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import uuid


@unittest.skipUnless(os.environ.get('GRAY_TEST_BIN'), 'Set GRAY_TEST_BIN')
class SessionMaintenanceTests(unittest.TestCase):
    def test_large_compacted_session_archived_before_provider_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);sessions=root/'sessions';sessions.mkdir()
            sid=str(uuid.uuid4());path=sessions/(sid+'.jsonl')
            header=dict(version=1,id=sid,timestamp=1,cwd=tmp,model='test')
            def row(i,text,boundary=False):
                return dict(entry_id=i,parent_id=i-1 if i else None,timestamp=i,
                            compaction_boundary=boundary,message=dict(role='user',content=[dict(type='text',text=text)]))
            with path.open('w') as f:
                f.write(json.dumps(header)+'\n')
                for i in range(35): f.write(json.dumps(row(i,'x'*1024*1024))+'\n')
                f.write(json.dumps(row(35,'boundary',True))+'\n')
                f.write(json.dumps(row(36,'retained summary'))+'\n')
            original=path.stat().st_size
            env={k:v for k,v in os.environ.items() if not k.startswith(('GRAY_','OPENAI_'))};env['GRAY_HOME']=tmp
            # No provider config: maintenance is local, no paid work necessary.
            result=subprocess.run([os.environ['GRAY_TEST_BIN'],'-p','hello','--json','--session',sid],
                cwd=tmp,env=env,capture_output=True,text=True,timeout=30)
            self.assertNotEqual(result.returncode,0)
            self.assertLess(path.stat().st_size,4096)
            archives=list((sessions/'archive').glob('*.jsonl'))
            self.assertEqual(len(archives),1)
            self.assertEqual(archives[0].stat().st_size,original)
            rows=[json.loads(l) for l in path.read_text().splitlines()]
            self.assertEqual(rows[0]['id'],sid)
            self.assertEqual(rows[1]['message']['content'][0]['text'],'retained summary')
            self.assertEqual(rows[1]['entry_id'],0)
            self.assertIsNone(rows[1]['parent_id'])
