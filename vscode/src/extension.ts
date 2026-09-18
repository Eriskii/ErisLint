import * as vscode from "vscode";
import { existsSync } from "node:fs";
import * as path from "node:path";
import { Evaluation, Plan, Report, editorOffset, functionsIn, hoverMarkdown, runCli } from "./protocol";

interface Action { uri: string; version: number; start: number }

function isAction(value: unknown): value is Action {
  if (!value || typeof value !== "object") return false;
  const action = value as Partial<Action>;
  return typeof action.uri === "string" && action.uri.length > 0
    && Number.isSafeInteger(action.version) && Number.isSafeInteger(action.start)
    && action.version! >= 1 && action.start! >= 0;
}

interface DocumentState {
  version: number;
  source: string;
  functions: Evaluation[];
  results: Map<number, Report>;
  running: Map<number, AbortController>;
}

export function activate(context: vscode.ExtensionContext) {
  const states = new Map<string, DocumentState>();
  const scans = new Map<string, { version: number; controller: AbortController; promise: Promise<DocumentState> }>();
  const changed = new vscode.EventEmitter<void>();
  const output = vscode.window.createOutputChannel("ErisLint");
  const selector = { language: "rust", scheme: "file" };
  const colors = ["editorInfo.foreground", "charts.green", "editorWarning.foreground", "editorError.foreground"];
  const decorations = colors.map(color => vscode.window.createTextEditorDecorationType({
    borderWidth: "0 0 1px 0", borderStyle: "dotted", borderColor: new vscode.ThemeColor(color),
    rangeBehavior: vscode.DecorationRangeBehavior.ClosedClosed,
  }));
  context.subscriptions.push(changed, output, ...decorations);

  const range = (document: vscode.TextDocument, source: string, span: { start: number; end: number }) =>
    new vscode.Range(document.positionAt(editorOffset(source, span.start)), document.positionAt(editorOffset(source, span.end)));

  function paint() {
    for (const editor of vscode.window.visibleTextEditors) {
      const state = states.get(editor.document.uri.toString());
      const buckets: vscode.Range[][] = [[], [], [], []];
      if (state?.version === editor.document.version && vscode.workspace.getConfiguration("erislint", editor.document.uri).get("highlightFunctions", true)) {
        for (const fn of state.functions) {
          const result = state.results.get(fn.location.start);
          const color = !result ? 0 : result.errors ? 3 : result.warnings ? 2 : 1;
          buckets[color].push(range(editor.document, state.source, fn.location));
        }
      }
      decorations.forEach((decoration, index) => editor.setDecorations(decoration, buckets[index]));
    }
  }

  function invalidate(uri?: string) {
    for (const key of new Set([...states.keys(), ...scans.keys()])) {
      if (uri && uri !== key) continue;
      scans.get(key)?.controller.abort();
      states.get(key)?.running.forEach(controller => controller.abort());
      scans.delete(key);
      states.delete(key);
    }
    changed.fire();
    paint();
  }

  function invocation(document: vscode.TextDocument) {
    const settings = vscode.workspace.getConfiguration("erislint", document.uri);
    const folder = vscode.workspace.getWorkspaceFolder(document.uri)?.uri.fsPath ?? path.dirname(document.uri.fsPath);
    const configured = settings.get<string>("binaryPath", "");
    const executable = process.platform === "win32" ? "erislint.exe" : "erislint";
    const local = ["debug", "release"].map(profile => path.join(folder, "target", profile, executable));
    const binary = configured
      ? (configured.includes("/") || configured.includes("\\") ? path.resolve(folder, configured) : configured)
      : local.find(existsSync) ?? executable;
    const config = settings.get<string>("configPath", "");
    const args = config ? ["--config", path.resolve(folder, config)] : [];
    return { binary, args, cwd: path.dirname(document.uri.fsPath) };
  }

  async function scan(document: vscode.TextDocument): Promise<DocumentState> {
    const uri = document.uri.toString();
    const existing = states.get(uri);
    if (existing?.version === document.version) return existing;
    const pending = scans.get(uri);
    if (pending?.version === document.version) return pending.promise;
    const version = document.version;
    const source = document.getText();
    const controller = new AbortController();
    const { binary, args, cwd } = invocation(document);
    const promise = runCli<Plan>(binary, [...args, "--dry-run", "--stdin-file", document.uri.fsPath], {
      cwd, source, signal: controller.signal,
    }).then(plan => {
      if (document.version !== version || controller.signal.aborted) throw new Error("Document changed.");
      const state: DocumentState = { version, source, functions: functionsIn(plan), results: new Map(), running: new Map() };
      states.set(uri, state);
      paint();
      return state;
    }).finally(() => { if (scans.get(uri)?.controller === controller) scans.delete(uri); });
    scans.set(uri, { version, controller, promise });
    return promise;
  }

  async function setKey(): Promise<string | undefined> {
    const key = await vscode.window.showInputBox({
      title: "Jev API key", prompt: "Stored in VS Code's encrypted secret storage.",
      password: true, ignoreFocusOut: true,
      validateInput: value => value.trim() ? undefined : "Enter a Jev API key.",
    });
    if (!key?.trim()) return undefined;
    await context.secrets.store("jev_key", key.trim());
    return key.trim();
  }

  async function runFunction(argument?: unknown) {
    const action = isAction(argument) ? argument : undefined;
    if (argument !== undefined && !(argument instanceof vscode.Uri) && !action) {
      throw new Error("Invalid function command. Use Run Current Function or a refreshed Run ErisLint button.");
    }
    const editor = vscode.window.activeTextEditor;
    const uri = action ? vscode.Uri.parse(action.uri) : argument instanceof vscode.Uri ? argument : editor?.document.uri;
    const document = uri ? await vscode.workspace.openTextDocument(uri) : undefined;
    if (!document || document.languageId !== "rust" || document.uri.scheme !== "file") {
      throw new Error("Open a Rust file to run ErisLint.");
    }
    const state = await scan(document);
    if (action && action.version !== state.version) throw new Error("The function changed. Use its refreshed Run ErisLint button.");
    const cursor = editor?.document.uri.toString() === document.uri.toString() ? editor.selection.active : undefined;
    const fn = action ? state.functions.find(fn => fn.location.start === action.start)
      : state.functions.filter(fn => cursor && range(document, state.source, fn.range).contains(cursor))
        .sort((a, b) => (a.range.end - a.range.start) - (b.range.end - b.range.start))[0];
    if (!fn) throw new Error("No configured function rules at the cursor.");
    const start = fn.location.start;
    if (state.running.has(start)) return;
    const key = await context.secrets.get("jev_key") || process.env.jev_key || await setKey();
    if (!key) return;
    if (document.version !== state.version || states.get(document.uri.toString()) !== state) return;
    const controller = new AbortController();
    state.running.set(start, controller);
    state.results.delete(start);
    changed.fire();
    paint();
    try {
      await vscode.window.withProgress({ location: vscode.ProgressLocation.Window, title: `ErisLint: ${fn.target}`, cancellable: true }, async (_progress, token) => {
        const cancellation = token.onCancellationRequested(() => controller.abort());
        try {
          const { binary, args, cwd } = invocation(document);
          const report = await runCli<Report>(binary, [...args, "--format", "json", "--stdin-file", document.uri.fsPath, "--target-start", String(start)], {
            cwd, source: state.source, key, signal: controller.signal,
          });
          if (!Array.isArray(report.answers) || report.answers.length === 0) throw new Error("No probabilities returned. Build the current ErisLint CLI.");
          if (document.version === state.version && !controller.signal.aborted) state.results.set(start, report);
        } finally { cancellation.dispose(); }
      });
    } catch (error) {
      if (!controller.signal.aborted) throw error;
    } finally {
      state.running.delete(start);
      changed.fire();
      paint();
    }
  }

  context.subscriptions.push(
    vscode.languages.registerCodeLensProvider(selector, {
      onDidChangeCodeLenses: changed.event,
      async provideCodeLenses(document) {
        try {
          const state = await scan(document);
          return state.functions.map(fn => {
            const start = fn.location.start;
            const result = state.results.get(start);
            const running = state.running.has(start);
            const title = running ? "Running ErisLint…" : result
              ? `↻ Run ErisLint · ${result.warnings} warnings, ${result.errors} errors · hover for probabilities`
              : `▶ Run ErisLint · ${Object.keys(fn.request.questions).length} rules`;
            return new vscode.CodeLens(range(document, state.source, fn.location), {
              title, command: running ? "" : "erislint.runFunction",
              arguments: [{ uri: document.uri.toString(), version: state.version, start } satisfies Action],
            });
          });
        } catch (error) {
          if (!(error instanceof Error && error.name === "AbortError")) output.appendLine(String(error));
          return [];
        }
      },
    }),
    vscode.languages.registerHoverProvider(selector, {
      provideHover(document, position) {
        const state = states.get(document.uri.toString());
        if (!state || state.version !== document.version) return;
        const fn = state.functions.find(fn => range(document, state.source, fn.location).contains(position));
        if (!fn) return;
        const report = state.results.get(fn.location.start);
        const markdown = new vscode.MarkdownString(report ? hoverMarkdown(report) : "Click **Run ErisLint** above this function to see every rule's choice probabilities.");
        markdown.isTrusted = false;
        markdown.supportHtml = false;
        return new vscode.Hover(markdown, range(document, state.source, fn.location));
      },
    }),
    vscode.commands.registerCommand("erislint.runFunction", async action => {
      try { await runFunction(action); }
      catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        output.appendLine(message);
        void vscode.window.showErrorMessage(`ErisLint: ${message}`);
      }
    }),
    vscode.commands.registerCommand("erislint.setJevKey", setKey),
    vscode.commands.registerCommand("erislint.clearJevKey", () => context.secrets.delete("jev_key")),
    vscode.commands.registerCommand("erislint.refresh", () => invalidate()),
    vscode.workspace.onDidChangeTextDocument(event => { if (event.contentChanges.length) invalidate(event.document.uri.toString()); }),
    vscode.workspace.onDidCloseTextDocument(document => invalidate(document.uri.toString())),
    vscode.workspace.onDidChangeConfiguration(event => { if (event.affectsConfiguration("erislint")) invalidate(); }),
    vscode.window.onDidChangeVisibleTextEditors(paint),
    { dispose: () => invalidate() },
  );
  const watcher = vscode.workspace.createFileSystemWatcher("**/*.json");
  context.subscriptions.push(watcher, watcher.onDidChange(() => invalidate()), watcher.onDidCreate(() => invalidate()), watcher.onDidDelete(() => invalidate()));
}
