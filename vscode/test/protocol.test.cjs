const { test } = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { editorOffset, functionsIn, hoverMarkdown, runCli } = require('../out/protocol');

test('converts Rust byte offsets to VS Code UTF-16 offsets', () => {
  const text = '// 🦀\r\nfn café() {}';
  const start = Buffer.byteLength(text.slice(0, text.indexOf('café')));
  const end = start + Buffer.byteLength('café');
  assert.equal(text.slice(editorOffset(text, start), editorOffset(text, end)), 'café');
});

test('one function button combines rules with different context requests', () => {
  const evaluation = (start, rule, kind = 'function') => ({
    kind, target: 'same_name', location: { start }, request: { questions: { [rule]: {} } },
  });
  const functions = functionsIn({ evaluations: [evaluation(4, 'a'), evaluation(4, 'b'), evaluation(40, 'a'), evaluation(0, 'file', 'file')] });
  assert.equal(functions.length, 2);
  assert.deepEqual(Object.keys(functions[0].request.questions), ['a', 'b']);
});

test('hover shows every option, descriptions, model, confidence, and no-diagnostic answers', () => {
  const hover = hoverMarkdown({ diagnostics: [], answers: [{
    rule: 'simplicity', model: 'jev-test',
    question: { instructions: 'Is it simple?', criteria: { yes: 'Simple', no: 'Complex', unknown: '[link](command:evil)|<html>' } },
    answer: { choice: 'yes', confidence: 0.85, probabilities: { yes: 0.9, no: 0.1, unknown: 0 } },
  }] });
  for (const text of ['90.00%', '10.00%', '0.00%', '85.00%', 'unknown', 'Simple', 'No diagnostic triggered.', 'jev\\-test']) assert.ok(hover.includes(text), text);
  assert.ok(!hover.includes('[link](command:evil)'));
  assert.ok(!hover.includes('<html>'));
});

test('CLI transport sends an unsaved snapshot and accepts lint-error exit status 1', async t => {
  const folder = mkdtempSync(path.join(tmpdir(), 'erislint-protocol-'));
  t.after(() => rmSync(folder, { recursive: true, force: true }));
  const script = path.join(folder, 'fake.cjs');
  writeFileSync(script, `let source = ''; process.stdin.on('data', x => source += x); process.stdin.on('end', () => { console.log(JSON.stringify({ source, keyPresent: !!process.env.jev_key, args: process.argv.slice(2) })); process.exitCode = 1; });`);
  const report = await runCli(process.execPath, [script, '--target-start', '4'], { cwd: folder, source: 'fn live_buffer() {}', key: 'test-only-key' });
  assert.equal(report.source, 'fn live_buffer() {}');
  assert.equal(report.keyPresent, true);
  assert.deepEqual(report.args, ['--target-start', '4']);
  const offline = await runCli(process.execPath, [script], { cwd: folder, source: 'fn offline() {}' });
  assert.equal(offline.keyPresent, false);
});

test('CLI transport redacts a key from process errors and supports cancellation', async () => {
  await assert.rejects(runCli(process.execPath, ['-e', 'console.error(process.env.jev_key); process.exit(2)'], { cwd: __dirname, source: '', key: 'never-print-this' }), error => {
    assert.ok(!error.message.includes('never-print-this'));
    return error.message.includes('[redacted]');
  });
  const controller = new AbortController();
  const run = runCli(process.execPath, ['-e', 'setTimeout(() => {}, 10000)'], { cwd: __dirname, source: '', signal: controller.signal });
  controller.abort();
  await assert.rejects(run, error => error.name === 'AbortError');
});
