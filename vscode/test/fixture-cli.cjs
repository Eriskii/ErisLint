#!/usr/bin/env node
// Copyright (C) 2026 Eriskii
// SPDX-License-Identifier: AGPL-3.0-only
// See LICENSE for the full license text.

// Extension-host fixture: real AST discovery, deterministic Jev answers, no network.
const { spawnSync } = require('node:child_process');
let source = '';
process.stdin.on('data', chunk => source += chunk);
process.stdin.on('end', () => {
  const args = process.argv.slice(2);
  const live = !args.includes('--dry-run');
  const result = spawnSync(process.env.ERISLINT_TEST_BINARY, live ? [...args, '--dry-run'] : args, {
    input: source, encoding: 'utf8', env: { ...process.env, jev_key: '' },
  });
  if (result.status !== 0) {
    process.stderr.write(result.stderr);
    process.exitCode = result.status ?? 2;
    return;
  }
  const plan = JSON.parse(result.stdout);
  if (!live) { console.log(JSON.stringify(plan)); return; }
  const answers = plan.evaluations.flatMap(evaluation => Object.entries(evaluation.request.questions).map(([rule, question]) => ({
    rule, question, target: evaluation.target, location: evaluation.location, model: 'jev-extension-test',
    answer: { type: 'choice', choice: 'bad', confidence: 0.7, probabilities: { good: 0.2, bad: 0.8, unknown: 0 } },
  })));
  console.log(JSON.stringify({ warnings: answers.length, errors: 0, answers,
    diagnostics: answers.map(answer => ({ rule: answer.rule, level: 'warn', message: 'Simplify this function.' })),
  }));
});
