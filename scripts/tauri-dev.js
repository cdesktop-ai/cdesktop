#!/usr/bin/env node

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');

const dotenvPath = path.join(root, '.env');
if (fs.existsSync(dotenvPath)) {
  for (const line of fs.readFileSync(dotenvPath, 'utf8').split(/\r?\n/)) {
    const match = line.match(/^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)\s*$/);
    if (!match || Object.hasOwn(process.env, match[1])) continue;
    let value = match[2].trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    } else {
      value = value.replace(/\s+#.*$/, '');
    }
    process.env[match[1]] = value;
  }
}

const { ports } = require('./dev-ports');

const env = {
  ...process.env,
  FRONTEND_PORT: String(ports.frontend),
  BACKEND_PORT: String(ports.backend),
  PREVIEW_PROXY_PORT: String(ports.preview_proxy),
  CDT_ALLOWED_ORIGINS: `http://localhost:${ports.frontend}`,
  DISABLE_WORKTREE_CLEANUP: '1',
  RUST_LOG: process.env.RUST_LOG || 'debug',
};
const config = JSON.stringify({
  build: { devUrl: `http://localhost:${ports.frontend}` },
});
const child = spawn(
  'cargo',
  ['tauri', 'dev', '--config', config],
  {
    cwd: path.join(root, 'crates/tauri-app'),
    env,
    stdio: 'inherit',
    windowsHide: true,
  }
);

child.on('error', (error) => {
  console.error(`Could not start Tauri development mode: ${error.message}`);
  process.exitCode = 1;
});
child.on('exit', (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exitCode = code ?? 1;
});
