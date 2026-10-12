// Real compiled WASM + a synthetic Tauri API, no browser or personal config.
const assert = require('node:assert/strict');
const path = require('node:path');
let calls = [];
let polls = 0;
let pollMode = 'success';
global.window = { __TAURI__: { core: { invoke: async (cmd, args) => {
  calls.push(cmd);
  switch (cmd) {
    case 'reject': throw 'backend unavailable';
    case 'reject_object': throw { diagnostic: 'unavailable' };
    case 'malformed': return { library_path: 7 };
    case 'valid': return { library_path: 'C:/Автор/Книга' };
    case 'scan_library': assert.equal(args.path, 'fixture/library'); return 42;
    case 'poll_job':
      assert.equal(args.id, 42);
      if (pollMode === 'reject') throw 'poll failed';
      if (pollMode === 'wrongId') return { id: 43, events: [], result: undefined };
      if (pollMode === 'failedJob') return { id: 42, events: [], result: { Err: 'conversion failed' } };
      return { id: 42, events: [], result: ++polls === 1 ? undefined : { Ok: { Scan: { books: [], warnings: ['fixture warning'] } } } };
    default: throw `Unexpected invocation ${cmd}`;
  }
} } } };
const wasm = require(path.resolve(__dirname, '../target/wasm-ipc/rulibre_gui.js'));
async function rejects(promise, expected) {
  try { await promise; assert.fail('expected rejection'); }
  catch (error) { assert.match(String(error), expected); }
}
(async () => {
  await rejects(wasm.smoke_config('reject'), /backend unavailable/);
  await rejects(wasm.smoke_config('reject_object'), /./);
  await rejects(wasm.smoke_config('malformed'), /string|invalid|type/i);
  assert.equal((await wasm.smoke_config('valid')).library_path, 'C:/Автор/Книга');
  const before = calls.length;
  await rejects(wasm.smoke_bad_args(), /fixture serialization failure/);
  assert.equal(calls.length, before);
  assert.deepEqual((await wasm.smoke_scan()).warnings, ['fixture warning']);
  assert.equal(polls, 2);
  pollMode = 'reject';
  await rejects(wasm.smoke_scan(), /poll failed/);
  pollMode = 'wrongId';
  await rejects(wasm.smoke_scan(), /Mismatched job/);
  pollMode = 'failedJob';
  await rejects(wasm.smoke_scan(), /conversion failed/);
  console.log('WASM IPC: rejection, object rejection, malformed response, Unicode, serialization failure and job polling passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
