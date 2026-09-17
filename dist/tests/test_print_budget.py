"""Verify enforcement inside a real agent tool loop, including unknown usage."""
import asyncio
import json
import os
from pathlib import Path
import tempfile
import unittest


@unittest.skipUnless(os.environ.get('GRAY_TEST_BIN'), 'Set GRAY_TEST_BIN')
class PrintBudgetTests(unittest.IsolatedAsyncioTestCase):
    async def test_request_and_spend_limits_stop_subsequent_requests(self):
        for flags, usage in [(['--max-requests','1'], None),
                            (['--max-cost-usd','0.000001','--input-price','1','--output-price','1'], {'prompt_tokens':10,'completion_tokens':2}),
                            (['--max-cost-usd','1','--input-price','1','--output-price','1'], None)]:
            with self.subTest(flags=flags, usage=usage):
                requests=[]
                async def serve(reader,writer):
                    header=await reader.readuntil(b'\r\n\r\n')
                    length=next(int(l.split(b':',1)[1]) for l in header.split(b'\r\n') if l.lower().startswith(b'content-length:'))
                    requests.append(json.loads(await reader.readexactly(length)))
                    delta=dict(tool_calls=[dict(index=0,id='call1',type='function',function=dict(name='no_such_tool',arguments='{}'))])
                    chunk=dict(id='fixture',object='chat.completion.chunk',created=1,model='test-model',
                               choices=[dict(index=0,delta=delta,finish_reason='tool_calls')])
                    if usage: chunk['usage']=usage
                    body=('data: '+json.dumps(chunk)+'\n\ndata: [DONE]\n\n').encode()
                    writer.write(b'HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: '+str(len(body)).encode()+b'\r\n\r\n'+body)
                    await writer.drain(); writer.close(); await writer.wait_closed()
                server=await asyncio.start_server(serve,'127.0.0.1',0)
                try:
                    with tempfile.TemporaryDirectory() as tmp:
                        root=Path(tmp)
                        (root/'config.json').write_text(json.dumps(dict(model='test-model',api_key='fixture',base_url=f'http://127.0.0.1:{server.sockets[0].getsockname()[1]}/v1',context_window=128000)))
                        (root/'gray.yml').write_text('plugins:\n  - builtin: tools-minimal\n')
                        env={k:v for k,v in os.environ.items() if not k.startswith(('GRAY_','OPENAI_'))};env['GRAY_HOME']=tmp
                        proc=await asyncio.create_subprocess_exec(os.environ['GRAY_TEST_BIN'],'-p','hello','--json',*flags,cwd=tmp,env=env,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.PIPE)
                        try: out,err=await asyncio.wait_for(proc.communicate(),30)
                        finally:
                            if proc.returncode is None: proc.kill();await proc.wait()
                        rows=[json.loads(l) for l in out.splitlines()]
                        self.assertNotEqual(proc.returncode,0,err.decode())
                        self.assertEqual(len(requests),1)
                        self.assertEqual(rows[-1]['type'],'error')
                        self.assertEqual(rows[-1]['accounting']['requests'],1)
                        self.assertEqual(rows[-1]['accounting']['usage_complete'],usage is not None)
                finally:
                    server.close();await server.wait_closed()
