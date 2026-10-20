#!/usr/bin/env node

const { spawn, execFile } = require('node:child_process');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const ports =
  process.env.FRONTEND_PORT &&
  process.env.BACKEND_PORT &&
  process.env.PREVIEW_PROXY_PORT
    ? {}
    : require('./dev-ports').ports;

const env = {
  ...process.env,
  FRONTEND_PORT: process.env.FRONTEND_PORT || String(ports.frontend),
  BACKEND_PORT: process.env.BACKEND_PORT || String(ports.backend),
  PREVIEW_PROXY_PORT:
    process.env.PREVIEW_PROXY_PORT || String(ports.preview_proxy),
  CDT_ALLOWED_ORIGINS:
    process.env.CDT_ALLOWED_ORIGINS ||
    `http://localhost:${process.env.FRONTEND_PORT || ports.frontend}`,
  DISABLE_WORKTREE_CLEANUP: process.env.DISABLE_WORKTREE_CLEANUP || '1',
  RUST_LOG: process.env.RUST_LOG || 'debug',
};
const windows = process.platform === 'win32';
let child;
let usingWatch = true;
let stopping = false;

function runCargo(args) {
  child = spawn('cargo', args, {
    cwd: root,
    env,
    stdio: 'inherit',
    windowsHide: true,
  });
  child.on('error', (error) => {
    console.error(`Could not start the Rust backend: ${error.message}`);
    process.exitCode = 1;
  });
  child.on('exit', (code, signal) => {
    if (stopping) return;
    if (usingWatch && code === 101) {
      usingWatch = false;
      console.warn('cargo-watch is unavailable; starting the backend without file watching.');
      runCargo(['run', '--bin', 'server']);
      return;
    }
    if (signal) process.kill(process.pid, signal);
    else process.exitCode = code ?? 1;
  });
}

function stop(signal) {
  stopping = true;
  if (!child || child.exitCode !== null || child.killed) return;
  if (windows && child.pid) {
    execFile('taskkill', ['/pid', String(child.pid), '/t', '/f'], {
      windowsHide: true,
    });
  } else {
    child.kill(signal);
  }
}

process.on('SIGINT', () => stop('SIGINT'));
process.on('SIGTERM', () => stop('SIGTERM'));
runCargo(['watch', '-w', 'crates', '-x', 'run --bin server']);
