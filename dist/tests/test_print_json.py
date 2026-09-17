"""Real print-mode NDJSON contract, loopback provider, no user credentials."""
import asyncio
import json
import os
from pathlib import Path
import tempfile
import unittest


@unittest.skipUnless(os.environ.get('GRAY_TEST_BIN'), 'Set GRAY_TEST_BIN')
class JsonPrintTests(unittest.IsolatedAsyncioTestCase):
    async def test_reply_session_resume_and_redacted_prompt(self):
        requests = []
        async def serve(reader, writer):
            header = await reader.readuntil(b'\r\n\r\n')
            length = next(int(l.split(b':',1)[1]) for l in header.split(b'\r\n') if l.lower().startswith(b'content-length:'))
            requests.append(json.loads(await reader.readexactly(length)))
            chunk = dict(id='fixture', object='chat.completion.chunk', created=1, model='test-model',
                choices=[dict(index=0, delta=dict(content='answer'), finish_reason='stop')],
                usage=dict(prompt_tokens=10, completion_tokens=2, total_tokens=12))
            body = ('data: '+json.dumps(chunk)+'\n\ndata: [DONE]\n\n').encode()
            writer.write(b'HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: '+str(len(body)).encode()+b'\r\n\r\n'+body)
            await writer.drain()
            writer.close()
            await writer.wait_closed()
        server = await asyncio.start_server(serve, '127.0.0.1', 0)
        self.addAsyncCleanup(server.wait_closed)
        self.addCleanup(server.close)
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            (home/'config.json').write_text(json.dumps(dict(model='test-model', api_key='fixture',
                base_url=f'http://127.0.0.1:{server.sockets[0].getsockname()[1]}/v1', context_window=128000)))
            (home/'gray.yml').write_text('plugins:\n  - builtin: tools-minimal\n')
            env = {k:v for k,v in os.environ.items() if not k.startswith(('GRAY_', 'OPENAI_', 'DISCORD_'))}
            env['GRAY_HOME'] = tmp
            async def run(*args):
                proc = await asyncio.create_subprocess_exec(os.environ['GRAY_TEST_BIN'], *args, cwd=tmp, env=env,
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
                out, err = await asyncio.wait_for(proc.communicate(), 40)
                return proc.returncode, [json.loads(l) for l in out.splitlines()], err.decode()
            code, rows, err = await run('-p', 'Remember fixture API_KEY=not-a-real-secret-123456789', '--json')
            self.assertEqual(code, 0, err)
            self.assertTrue(rows)
            final = rows[-1]
            self.assertEqual(final['type'], 'result')
            self.assertEqual(final['text'], 'answer')
            self.assertTrue(final['session_id'])
            self.assertTrue(final['turn_id'])
            self.assertTrue(all(r['protocol'] == 1 for r in rows))
            code, resumed, err = await run('-p', 'Follow up', '--json', '--session', final['session_id'])
            self.assertEqual(code, 0, err)
            self.assertEqual(resumed[-1]['session_id'], final['session_id'])
            self.assertNotEqual(resumed[-1]['turn_id'], final['turn_id'])
            self.assertEqual(len(requests), 2)
            self.assertIn('answer', json.dumps(requests[-1]['messages']))
            code, rows, err = await run('-p', 'x', '--json', '--session', 'missing')
            self.assertNotEqual(code, 0)
            self.assertEqual(rows[-1]['type'], 'error')
            self.assertNotIn('not-a-real-secret', json.dumps(rows))
