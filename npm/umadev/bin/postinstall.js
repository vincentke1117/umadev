#!/usr/bin/env node
// SPDX-License-Identifier: MIT
//
// Post-install PATH check — GLOBAL installs only.
//
// Some node setups (notably Homebrew's node) leave npm's global bin dir OFF
// $PATH. The package installs fine and the `umadev` command is linked, but it
// lands in a directory the shell doesn't search, so the user later sees a bare
// "command not found" with no clue why. We cannot change the user's PATH from
// here, but we can detect the situation and print exactly what happened + how
// to fix it.
//
// FAIL-OPEN BY CONTRACT: every path through this script ends with exit code 0,
// and the whole body is wrapped in try/catch. A cosmetic PATH check must NEVER
// break `npm install` — if anything is uncertain, we stay silent.
'use strict';

function main() {
  // Only a global install (`npm i -g`) puts a command on PATH. For a local /
  // dependency install the command is not meant to be on PATH — say nothing.
  if (process.env.npm_config_global !== 'true') return;

  const path = require('node:path');

  // The prefix npm links global commands under. If npm didn't expose it we
  // can't reason about PATH — stay quiet rather than risk a false warning.
  const prefix = process.env.npm_config_prefix;
  if (!prefix) return;

  const isWin = process.platform === 'win32';
  // npm links global commands into <prefix>/bin on unix, <prefix> on Windows.
  const binDir = isWin ? prefix : path.join(prefix, 'bin');

  const sep = isWin ? ';' : ':';
  const strip = (p) => p.replace(/[\\/]+$/, '');
  const fold = (p) => (isWin ? strip(p).toLowerCase() : strip(p));
  const target = fold(binDir);
  const onPath = (process.env.PATH || '')
    .split(sep)
    .filter(Boolean)
    .some((p) => fold(p) === target);

  // All good — the command will be found. Don't add noise to a clean install.
  if (onPath) return;

  const w = (s) => process.stderr.write(s + '\n');
  w('');
  w('  [warn] umadev installed OK, but its command directory is NOT on your PATH:');
  w('           ' + binDir);
  w('         so running `umadev` will say "command not found".');
  w('         This is your npm/shell setup (common with Homebrew node), not a umadev bug.');
  w('         umadev 已装好,但命令目录不在 PATH 上,所以敲 umadev 会提示找不到。');
  w('');
  w('  Fix — point npm at a prefix already on your PATH, then reinstall:');
  w('           npm config set prefix ~/.npm-global');
  w('           # ensure ~/.npm-global/bin is on your PATH, then:');
  w('           npm i -g umadev@latest');
  w('         …or just add the directory above to your PATH:');
  w('           export PATH="' + binDir + (isWin ? ';%PATH%"' : ':$PATH"'));
  w('');
}

try {
  main();
} catch (_e) {
  // Never fail the install over a cosmetic PATH check.
}
