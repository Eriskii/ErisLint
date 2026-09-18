# ErisLint for VS Code

Rust function declarations get a **▶ Run ErisLint** button and a dotted underline.
Click the button, then hover the function name to see **every option's probability
for every matching rule**, the selected choice, confidence, and any warning/error.
Passing rules appear too. Completed functions are underlined green, yellow, or red
according to their diagnostics.

Only clicking Run sends code to Jev. Function discovery is local. Runs use the
current editor buffer without saving it; editing the document clears stale
results and cancels its outstanding runs.

## Setup

1. Build the ErisLint CLI from the parent directory with `cargo build --locked`.
2. Install the packaged `.vsix` using **Extensions → … → Install from VSIX**.
3. Open a Rust project containing `erislint.json` and function rules.
4. Run **ErisLint: Set Jev API Key** from the command palette, or launch VS Code
   with `jev_key` in its environment.
5. Click **Run ErisLint** above a function and hover its name after the run.

The key is saved in VS Code's encrypted secret storage. A saved key takes
precedence over the environment. **ErisLint: Clear Saved Jev API Key** removes it.
The key is passed to the CLI through its environment, never as a command argument.

The extension discovers a CLI in the workspace's `target/debug` or
`target/release` directory, then falls back to `erislint` on PATH. For another
installation, set **ErisLint: Binary Path** to the executable. The extension does
not bundle a platform-specific CLI.

`erislint.configPath` selects an explicit configuration. Paths in settings are
relative to the workspace folder unless absolute. Otherwise configuration is
discovered from the Rust file's directory. `erislint.highlightFunctions` toggles
the underlines. CodeLens must be enabled (`editor.codeLens`) to show Run buttons.
**ErisLint: Run Current Function** also works from the command palette or editor
context menu with the cursor inside a function.

Functions need matching enabled rules to receive a button. This uses ErisLint's
Rust AST; methods and nested functions work without depending on rust-analyzer's
VS Code extension. Files must have a saved filesystem path, but their contents
may have unsaved changes. Fix syntax errors to restore function discovery; setup
and parse details appear in **Output → ErisLint**. JSON configuration changes
refresh the buttons automatically; **ErisLint: Refresh Functions** forces a refresh.

## Develop / package

```sh
npm ci
npm test
npm run test:host
npm run package
```

Install `erislint-0.1.1.vsix` locally. No Marketplace account is required. The
extension runs on the workspace side, so remote workspaces need the CLI there.
The extension-host test needs a built debug CLI, the `code` command, and a desktop
session. It opens an isolated test window and uses fixture answers without any
Jev requests or real credentials.

## License

Copyright (C) 2026 [Eriskii](https://github.com/Eriskii).

Licensed under the GNU Affero General Public License, version 3 only
(`AGPL-3.0-only`). See [LICENSE](LICENSE). The extension's source is available in
the [ErisLint repository](https://github.com/Eriskii/ErisLint/tree/main/vscode).
