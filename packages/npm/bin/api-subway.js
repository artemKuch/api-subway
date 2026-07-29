#!/usr/bin/env node

'use strict';

const { spawn } = require('node:child_process');

const targetPackages = {
  'darwin-arm64': '@api-subway/darwin-arm64',
  'darwin-x64': '@api-subway/darwin-x64',
  'linux-arm64': '@api-subway/linux-arm64',
  'linux-x64': '@api-subway/linux-x64',
  'win32-x64': '@api-subway/win32-x64',
};

const target = `${process.platform}-${process.arch}`;
const executableName = process.platform === 'win32' ? 'api-subway.exe' : 'api-subway';
let executable = process.env.API_SUBWAY_BINARY;

if (!executable) {
  const packageName = targetPackages[target];
  if (!packageName) {
    console.error(`api-subway does not publish a binary for ${target}.`);
    process.exit(2);
  }
  try {
    executable = require.resolve(`${packageName}/bin/${executableName}`);
  } catch {
    console.error(
      `The optional package ${packageName} is missing. Reinstall api-subway without disabling optional dependencies.`,
    );
    process.exit(2);
  }
}

const child = spawn(executable, process.argv.slice(2), { stdio: 'inherit' });
const forwardedSignals = process.platform === 'win32'
  ? ['SIGINT', 'SIGTERM']
  : ['SIGHUP', 'SIGINT', 'SIGTERM'];
const signalHandlers = new Map();

const removeSignalHandlers = () => {
  for (const [signal, handler] of signalHandlers) {
    process.off(signal, handler);
  }
};

for (const signal of forwardedSignals) {
  const handler = () => {
    if (child.exitCode === null && child.signalCode === null) child.kill(signal);
  };
  signalHandlers.set(signal, handler);
  process.on(signal, handler);
}

child.once('error', (error) => {
  removeSignalHandlers();
  console.error(`api-subway failed to start: ${error.message}`);
  process.exit(2);
});

child.once('exit', (code, signal) => {
  removeSignalHandlers();
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 2);
});
