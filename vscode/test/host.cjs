const assert = require('node:assert/strict');
const path = require('node:path');
const vscode = require('vscode');

exports.run = async () => {
  const folder = vscode.workspace.workspaceFolders[0].uri.fsPath;
  await vscode.workspace.getConfiguration('erislint').update('binaryPath', path.join(__dirname, 'fixture-cli.cjs'), vscode.ConfigurationTarget.Workspace);
  await vscode.extensions.getExtension('erislint-local.erislint').activate();
  const document = await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(folder, 'src/lib.rs')));
  const editor = await vscode.window.showTextDocument(document);
  const lenses = (await vscode.commands.executeCommand('vscode.executeCodeLensProvider', document.uri))
    .filter(lens => lens.command?.command === 'erislint.runFunction');
  assert.equal(lenses.length, 2, 'AST declarations receive Run buttons');
  const lens = lenses.find(lens => document.getText(lens.range) === 'café');
  assert.ok(lens, 'Unicode declaration has the correct range');
  // VS Code passes a URI to editor context-menu commands, unlike CodeLens payloads.
  editor.selection = new vscode.Selection(lens.range.end, lens.range.end);
  for (const args of [[document.uri], [], lens.command.arguments]) {
    await vscode.commands.executeCommand('erislint.refresh');
    await vscode.commands.executeCommand(lens.command.command, ...args);
    const results = await vscode.commands.executeCommand('vscode.executeHoverProvider', document.uri, lens.range.start);
    assert.ok(results.flatMap(hover => hover.contents).some(content => content.value?.includes('80.00%')),
      `run produces probabilities for ${args.length ? (args[0] instanceof vscode.Uri ? 'context menu' : 'CodeLens') : 'command palette'}`);
  }
  const hovers = await vscode.commands.executeCommand('vscode.executeHoverProvider', document.uri, lens.range.start);
  const text = hovers.flatMap(hover => hover.contents.map(content => content.value ?? content)).join('\n');
  for (const expected of ['quality', '80.00%', '20.00%', '0.00%', '70.00%', 'jev\\-extension\\-test']) {
    assert.ok(text.includes(expected), `hover contains ${expected}`);
  }
  const completed = await vscode.commands.executeCommand('vscode.executeCodeLensProvider', document.uri);
  assert.ok(completed.some(lens => lens.command?.title.includes('1 warnings')));
  await editor.edit(edit => edit.insert(new vscode.Position(0, 0), '// unsaved edit\n'));
  const after = await vscode.commands.executeCommand('vscode.executeHoverProvider', document.uri, new vscode.Position(lens.range.start.line + 1, lens.range.start.character));
  assert.ok(!after.flatMap(hover => hover.contents).some(content => content.value?.includes('80.00%')), 'edits invalidate old probabilities');
  const refreshed = await vscode.commands.executeCommand('vscode.executeCodeLensProvider', document.uri);
  assert.equal(refreshed.filter(lens => lens.command?.command === 'erislint.runFunction').length, 2, 'buttons work with unsaved buffers');
  console.log('ERISLINT_EXTENSION_HOST_TESTS_PASSED');
};
