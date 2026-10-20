#!/usr/bin/env node

const { execFileSync, spawnSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const toolchainFile = fs.readFileSync(path.join(root, 'rust-toolchain.toml'), 'utf8');
const requiredToolchain = toolchainFile.match(/channel\s*=\s*"([^"]+)"/)?.[1];
let hasErrors = false;

function report(name, ok, details, optional = false) {
  const state = ok ? 'OK' : optional ? 'OPTIONAL' : 'MISSING';
  console.log(`[${state}] ${name}: ${details}`);
  if (!ok && !optional) hasErrors = true;
}

const nodeMajor = Number(process.versions.node.split('.')[0]);
report('Node.js', nodeMajor >= 20, `v${process.versions.node} (required >=20)`);

const pnpmVersion = process.env.npm_config_user_agent?.match(/pnpm\/(\d+\.\d+\.\d+)/)?.[1];
report(
  'pnpm',
  !!pnpmVersion && Number(pnpmVersion.split('.')[0]) >= 8,
  pnpmVersion ? `v${pnpmVersion} (required >=8)` : 'run setup-windows.ps1 to check/install (required >=8)'
);

let installedToolchains = '';
try {
  installedToolchains = execFileSync('rustup', ['toolchain', 'list'], {
    cwd: root,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'ignore'],
  });
} catch {
  // Report the missing toolchain with an actionable setup command.
}
report(
  `Rust toolchain ${requiredToolchain}`,
  !!requiredToolchain && installedToolchains.split(/\r?\n/).some((line) => line.startsWith(requiredToolchain)),
  requiredToolchain || 'channel not found in rust-toolchain.toml'
);

const cargoList = spawnSync('cargo', ['--list'], {
  cwd: root,
  encoding: 'utf8',
  stdio: ['ignore', 'pipe', 'ignore'],
});
report(
  'cargo-watch',
  cargoList.status === 0 && /\bwatch\b/.test(cargoList.stdout || ''),
  'optional; without it, backend runs without hot reload',
  true
);

if (process.platform === 'win32') {
  console.log('[INFO] Python: not required to start FreeClaude Voice.');
}

process.exitCode = hasErrors ? 1 : 0;
