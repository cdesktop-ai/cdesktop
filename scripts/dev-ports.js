const { execFileSync } = require('node:child_process');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const output = execFileSync(
  process.execPath,
  [path.join(__dirname, 'setup-dev-environment.js'), 'get'],
  { cwd: root, encoding: 'utf8' }
);
const jsonLine = output.trim().split(/\r?\n/).at(-1);

module.exports = { root, ports: JSON.parse(jsonLine) };
