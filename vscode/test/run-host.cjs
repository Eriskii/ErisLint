const { mkdtempSync, mkdirSync, writeFileSync, readdirSync, readFileSync, chmodSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const directory = mkdtempSync(path.join(tmpdir(), 'erislint-vscode-host-'));
const workspace = path.join(directory, 'project');
const userData = path.join(directory, 'user');
mkdirSync(path.join(workspace, 'src'), { recursive: true });
writeFileSync(path.join(workspace, 'src/lib.rs'), '// 🦀\nfn first() {}\nfn café() {}\n');
writeFileSync(path.join(workspace, 'erislint.json'), JSON.stringify({ rules: [{
  id: 'quality', where: { kind: 'function' },
  question: { type: 'choice', instructions: 'Is this simple?', criteria: { good: 'Simple', bad: 'Complex', unknown: 'Uncertain' } },
  diagnostics: [{ when: { choice: 'bad' }, level: 'warn', message: 'Simplify this function.' }],
}] }));
chmodSync(path.join(__dirname, 'fixture-cli.cjs'), 0o755);
const env = { ...process.env, jev_key: 'erislint-test-key', ERISLINT_TEST_BINARY: path.resolve(__dirname, '../../target/debug/erislint') };
delete env.VSCODE_IPC_HOOK_CLI;
const result = spawnSync('code', [
  '--user-data-dir', userData, '--extensions-dir', path.join(directory, 'extensions'),
  `--extensionDevelopmentPath=${path.resolve(__dirname, '..')}`,
  `--extensionTestsPath=${path.join(__dirname, 'host.cjs')}`,
  '--disable-extensions', '--disable-workspace-trust', '--skip-welcome', '--skip-release-notes', '--new-window', '--wait', workspace,
], { env, stdio: 'inherit', timeout: 60_000 });
const logs = path.join(userData, 'logs');
const passed = !result.error && result.status === 0 && readdirSync(logs, { recursive: true })
  .filter(name => name.endsWith('renderer.log'))
  .some(name => readFileSync(path.join(logs, name), 'utf8').includes('ERISLINT_EXTENSION_HOST_TESTS_PASSED'));
if (!passed) throw new Error(`VS Code integration test failed; inspect ${directory}`);
console.log('VS Code integration passed: context menu, command palette, function buttons, all probabilities, Unicode ranges, and edit invalidation.');
rmSync(directory, { recursive: true, force: true });
