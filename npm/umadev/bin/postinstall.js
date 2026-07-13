#!/usr/bin/env node
// SPDX-License-Identifier: MIT
//
// Post-install guidance — the three ways an install ends up "installed but not
// working", each of which is an npm/OS setup fact rather than a umadev bug, and
// each of which we can name precisely at the exact moment it happens:
//
//   1. LOCAL install (`npm i umadev`, no `-g`). npm deliberately does not put a
//      locally-installed command on PATH, so the user types `umadev`, gets
//      "command not found", and concludes the package is broken. It isn't — it
//      is reached via `npx umadev`. We say so.
//   2. GLOBAL install run under `sudo`. It works today, but it leaves a
//      ROOT-OWNED tree in the npm prefix; every later NON-root npm command on
//      that prefix (including the tree-wide `npm update -g`) then fails with
//      EACCES and npm aborts the whole transaction — which wedges the user's
//      OTHER global packages (their base CLI) too. We name the sudo-free path.
//   3. GLOBAL install whose bin dir is off $PATH (common with Homebrew node).
//      The command is linked, just into a directory the shell doesn't search.
//
// FAIL-OPEN BY CONTRACT: every path through this script ends with exit code 0,
// and the whole body is wrapped in try/catch. Cosmetic guidance must NEVER
// break `npm install` — if anything is uncertain, we stay silent.
//
// REACH, HONESTLY: npm >= 7 SWALLOWS lifecycle-script output — what we print
// here is only surfaced by `npm i --foreground-scripts` (and by yarn / pnpm,
// which do stream it). We must NOT exit nonzero to force it out; that would fail
// the user's install. So this script is a BONUS channel, never the only one:
// the load-bearing copies of this guidance live where they always reach the
// user — `bin/cli.js` (runs with inherited stdio on every launch; warns once
// about a root-owned `sudo` install) and `umadev doctor` (the "npm install
// health" check), plus the READMEs.
'use strict';

const w = (s) => process.stderr.write(s + '\n');

// (1) A local install works fine — it just isn't on PATH, BY DESIGN. Tell the
// user how to actually run it instead of letting "command not found" read as a
// broken package. This is also the sudo-free path on Linux, so it's the first
// thing a user hitting the EACCES wall should hear about.
function localInstallHint() {
  w('');
  w('  umadev installed as a LOCAL dependency (no `-g`).');
  w('  npm does not put a local command on PATH — that is npm, not a bug. Run it with:');
  w('');
  w('      npx umadev');
  w('');
  w('  Want a plain `umadev` command WITHOUT sudo? Use a user-owned npm prefix:');
  w('      npm config set prefix ~/.npm-global');
  w('      export PATH="$HOME/.npm-global/bin:$PATH"   # add to ~/.zshrc or ~/.bashrc');
  w('      npm i -g umadev');
  w('  本地安装已完成。npm 不会把本地命令挂到 PATH 上,请用 `npx umadev` 运行。');
  w('');
}

// (2) Installed as root (`sudo npm i -g`). The install itself succeeds, so npm
// says nothing — but the tree it just wrote is root-owned, and that is a trap
// that fires LATER, on the user's other packages. Name it now, while the user
// is still looking at the terminal.
function sudoWarning(binDir) {
  w('');
  w('  [warn] umadev was installed with `sudo` (as root). It will run — but the files npm');
  w('         just wrote are ROOT-OWNED, and that breaks LATER npm commands you run as');
  w('         yourself: `npm update -g` and `npm i -g <anything>` on this prefix will fail');
  w('         with EACCES, and npm aborts the whole transaction — so your OTHER global');
  w('         packages (e.g. @anthropic-ai/claude-code, @openai/codex) can no longer be');
  w('         updated either. This is the classic `sudo npm` footgun, not a umadev bug.');
  w('         用 sudo 装会留下 root 属主的文件,之后你以普通用户跑 npm 会 EACCES,连带影响其它全局包。');
  w('');
  w('  Recommended — reinstall WITHOUT sudo, into a prefix you own:');
  w('           sudo npm uninstall -g umadev');
  w('           npm config set prefix ~/.npm-global');
  w('           export PATH="$HOME/.npm-global/bin:$PATH"   # add to ~/.zshrc or ~/.bashrc');
  w('           npm i -g umadev');
  w('');
  w('  If a past `sudo npm` already left root-owned files behind, repair them with:');
  w('           sudo chown -R $(whoami) ~/.npm ' + (binDir ? binDir.replace(/\/bin$/, '') : ''));
  w('');
}

function main() {
  // Only a global install (`npm i -g`) puts a command on PATH. A local install
  // is fine but invisible — point it at `npx` (case 1) and stop.
  if (process.env.npm_config_global !== 'true') {
    localInstallHint();
    return;
  }

  const path = require('node:path');

  // (2) Root-owned global install. `process.getuid` is unix-only; SUDO_USER is
  // set by sudo itself, so uid 0 + SUDO_USER is an unambiguous "a real user ran
  // sudo" (as opposed to a legitimately root-only box / a root container, where
  // this advice would be noise).
  if (
    typeof process.getuid === 'function' &&
    process.getuid() === 0 &&
    process.env.SUDO_USER
  ) {
    const p = process.env.npm_config_prefix;
    sudoWarning(p ? path.join(p, 'bin') : '');
    return;
  }

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
