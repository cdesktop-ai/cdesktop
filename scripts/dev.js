#!/usr/bin/env node

const { spawn, execFile, spawnSync } = require('node:child_process');
const http = require('node:http');
const path = require('node:path');
const { root, ports } = require('./dev-ports');
const prerequisiteCheck = spawnSync(
  process.execPath,
  [path.join(__dirname, 'check-windows-prerequisites.js')],
  { cwd: root, env: process.env, encoding: 'utf8' }
);
if (prerequisiteCheck.stdout) process.stdout.write(prerequisiteCheck.stdout);
if (prerequisiteCheck.stderr) process.stderr.write(prerequisiteCheck.stderr);
const prerequisitesReady = prerequisiteCheck.status === 0;

const env = {
  ...process.env,
  FRONTEND_PORT: String(ports.frontend),
  BACKEND_PORT: String(ports.backend),
  PREVIEW_PROXY_PORT: String(ports.preview_proxy),
  CDT_ALLOWED_ORIGINS: [
    `http://localhost:${ports.frontend}`,
    'https://10.126.127.4:4000',
  ].join(','),
  DISABLE_WORKTREE_CLEANUP: '1',
  RUST_LOG: process.env.RUST_LOG || 'debug',
};

const windows = process.platform === 'win32';
const pnpm = windows ? 'pnpm.cmd' : 'pnpm';
const children = [];
let stopping = false;

function start(label, command, args, cwd) {
  const child = spawn(command, args, {
    cwd,
    env,
    shell: windows && command.endsWith('.cmd'),
    stdio: ['inherit', 'pipe', 'pipe'],
  });
  children.push(child);
  child.stdout.on('data', (chunk) => process.stdout.write(`[${label}] ${chunk}`));
  child.stderr.on('data', (chunk) => process.stderr.write(`[${label}] ${chunk}`));
  child.on('error', (error) => {
    console.error(`[${label}] Could not start ${command}: ${error.message}`);
    stop(1);
  });
  child.on('exit', (code, signal) => {
    if (!stopping) {
      console.error(`[${label}] stopped (${signal || `exit ${code ?? 'unknown'}`}).`);
      if (label !== 'backend') stop(code || 1);
    }
  });
  return child;
}

function stop(exitCode = 0) {
  if (stopping) return;
  stopping = true;
  for (const child of children) {
    if (child.exitCode !== null || child.killed) continue;
    if (windows && child.pid) {
      execFile('taskkill', ['/pid', String(child.pid), '/t', '/f'], {
        windowsHide: true,
      });
    } else {
      child.kill();
    }
  }
  setTimeout(() => process.exit(exitCode), 500);
}

process.on('SIGINT', () => stop(0));
process.on('SIGTERM', () => stop(0));

console.log(`Frontend: http://localhost:${ports.frontend}`);
console.log(`Backend:  http://127.0.0.1:${ports.backend}/health`);

function openBrowserWhenReady() {
  if (!windows) return;
  const url = `http://localhost:${ports.frontend}`;
  const deadline = Date.now() + 60000;
  const probe = () => {
    if (stopping || Date.now() >= deadline) return;
    const request = http.get(url, (response) => {
      response.resume();
      if (response.statusCode && response.statusCode < 500) {
        execFile(
          'cmd.exe',
          ['/c', 'start', '', url],
          { windowsHide: true },
          (error) => {
            if (error) console.error(`Could not open browser: ${error.message}`);
          }
        );
        return;
      }
      setTimeout(probe, 700);
    });
    request.setTimeout(1000, () => request.destroy());
    request.on('error', () => setTimeout(probe, 700));
  };
  probe();
}

if (prerequisitesReady) {
  start('backend', process.execPath, [path.join(__dirname, 'backend-dev.js')], root);
} else {
  console.error(
    'Backend not started: run powershell -NoProfile -ExecutionPolicy RemoteSigned -File .\\scripts\\setup-windows.ps1 and choose automatic install or the manual guide.'
  );
}
start('frontend', pnpm, ['run', 'dev', '--port', String(ports.frontend)], path.join(root, 'packages/local-web'));
openBrowserWhenReady();
