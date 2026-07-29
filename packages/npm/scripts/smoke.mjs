import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const binary = process.env.API_SUBWAY_TEST_BINARY;
if (!binary) {
  console.error('API_SUBWAY_TEST_BINARY must point to a built api-subway binary.');
  process.exit(2);
}
const launcher = path.join(packageRoot, 'bin/api-subway.js');
const result = spawnSync(process.execPath, [launcher, '--help'], {
  env: { ...process.env, API_SUBWAY_BINARY: binary },
  stdio: 'inherit',
});
if (result.status !== 0) process.exit(result.status ?? 2);

if (process.platform !== 'win32') {
  const signaled = spawnSync(
    process.execPath,
    [launcher, '-e', "process.kill(process.pid, 'SIGTERM')"],
    {
      env: { ...process.env, API_SUBWAY_BINARY: process.execPath },
      stdio: 'ignore',
    },
  );
  if (signaled.signal !== 'SIGTERM') {
    console.error(`api-subway did not propagate SIGTERM (received ${signaled.signal ?? signaled.status}).`);
    process.exit(2);
  }
}
