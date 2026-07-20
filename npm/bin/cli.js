#!/usr/bin/env node
'use strict';

const fs = require('fs');
const { spawnSync } = require('child_process');
const { binaryPath } = require('../lib/platform');
const { ensureBinary } = require('../lib/download');

async function main() {
  const bin = binaryPath();

  // If postinstall was skipped (e.g. `npm install --ignore-scripts`, or an
  // `npx` invocation that bypassed scripts), fetch the binary on first run.
  if (!fs.existsSync(bin)) {
    await ensureBinary();
  }

  const res = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' });

  if (res.error) {
    console.error(`smolsonic: ${res.error.message}`);
    process.exit(1);
  }
  if (res.signal) {
    // Re-raise the terminating signal so exit status reflects reality.
    process.kill(process.pid, res.signal);
    return;
  }
  process.exit(res.status === null ? 1 : res.status);
}

main().catch((err) => {
  console.error(`smolsonic: ${err && err.message ? err.message : err}`);
  process.exit(1);
});
