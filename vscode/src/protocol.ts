import { spawn } from "node:child_process";

export interface Span { start: number; end: number; line: number; column: number }
export interface Location extends Span { file: string }
export interface Question { type: "choice"; instructions: string; criteria: Record<string, string> }
export interface Evaluation {
  location: Location;
  range: Span;
  target: string;
  kind: string;
  request: { questions: Record<string, Question> };
}
export interface Plan { evaluations: Evaluation[] }
export interface Answer {
  rule: string;
  target: string;
  location: Location;
  model: string;
  question: Question;
  answer: { choice: string; confidence: number; probabilities: Record<string, number> };
}
export interface Report {
  warnings: number;
  errors: number;
  answers: Answer[];
  diagnostics: { rule: string; level: "warn" | "error"; message: string }[];
}

export function functionsIn(plan: Plan): Evaluation[] {
  const functions = new Map<number, Evaluation>();
  for (const evaluation of plan.evaluations) {
    if (evaluation.kind !== "function") continue;
    const previous = functions.get(evaluation.location.start);
    functions.set(evaluation.location.start, previous ? {
      ...previous,
      request: { questions: { ...previous.request.questions, ...evaluation.request.questions } },
    } : evaluation);
  }
  return [...functions.values()];
}

// Rust spans count UTF-8 bytes; VS Code offsets count UTF-16 code units.
export function editorOffset(text: string, byteOffset: number): number {
  return Buffer.from(text, "utf8").subarray(0, byteOffset).toString("utf8").length;
}

function escape(text: string): string {
  return text.replace(/[\\`*_{}\[\]()#+.!|<>~\-]/g, "\\$&").replace(/\r?\n/g, " ");
}

const percent = (probability: number) => `${(probability * 100).toFixed(2)}%`;

export function hoverMarkdown(report: Report): string {
  return report.answers.map(result => {
    const { answer, question } = result;
    const rows = Object.entries(answer.probabilities)
      .sort(([a, p], [b, q]) => q - p || a.localeCompare(b))
      .map(([choice, probability]) => {
        const label = `${choice === answer.choice ? "✓ " : ""}${escape(choice)}`;
        return `| ${label} | ${percent(probability)} | ${escape(question.criteria[choice] ?? "")} |`;
      });
    const diagnostic = report.diagnostics.find(diagnostic => diagnostic.rule === result.rule);
    return [
      `### ${escape(result.rule)}`,
      escape(question.instructions),
      `**Selected:** ${escape(answer.choice)} · **Confidence:** ${percent(answer.confidence)}`,
      `| Option | Probability | Meaning |\n| :--- | ---: | :--- |\n${rows.join("\n")}`,
      diagnostic ? `**${diagnostic.level === "error" ? "Error" : "Warning"}:** ${escape(diagnostic.message)}` : "No diagnostic triggered.",
      `Model: ${escape(result.model)}`,
    ].join("\n\n");
  }).join("\n\n---\n\n");
}

export interface Invocation {
  cwd: string;
  source: string;
  key?: string;
  signal?: AbortSignal;
}

export function runCli<T>(binary: string, args: string[], options: Invocation): Promise<T> {
  return new Promise((resolve, reject) => {
    const env = { ...process.env };
    delete env.jev_key;
    if (options.key) env.jev_key = options.key;
    const child = spawn(binary, args, {
      cwd: options.cwd, env, shell: false, windowsHide: true,
      stdio: ["pipe", "pipe", "pipe"], signal: options.signal,
    });
    const output: Buffer[] = [];
    let size = 0;
    let error = "";
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error("ErisLint timed out. Try running the function again."));
    }, 180_000);
    const redact = (text: string) => options.key ? text.replaceAll(options.key, "[redacted]") : text;
    child.stdout.on("data", (data: Buffer) => {
      size += data.length;
      if (size > 64 * 1024 * 1024) {
        child.kill();
        reject(new Error("ErisLint output exceeded 64 MiB."));
      } else output.push(data);
    });
    child.stderr.on("data", (data: Buffer) => { error = (error + data.toString()).slice(-16_384); });
    child.once("error", failure => {
      clearTimeout(timer);
      reject(failure.name === "AbortError" ? failure : new Error(`Cannot start ErisLint: ${redact(failure.message)}. Build the CLI or set erislint.binaryPath.`));
    });
    child.once("close", code => {
      clearTimeout(timer);
      if (code !== 0 && code !== 1) {
        reject(new Error(redact(error.trim()) || `ErisLint exited with status ${code}.`));
        return;
      }
      try { resolve(JSON.parse(Buffer.concat(output).toString("utf8")) as T); }
      catch { reject(new Error("ErisLint did not return JSON. Build the current CLI and check erislint.binaryPath.")); }
    });
    // An early process error can close stdin before the snapshot finishes writing.
    child.stdin.on("error", () => {});
    child.stdin.end(options.source);
  });
}
