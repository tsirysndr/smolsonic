'use strict';

// Fetch the prebuilt smolsonic binary from the matching GitHub release and add
// it to this package. Failures here are non-fatal: the launcher (bin/cli.js)
// retries the download on first run, so a transient network error at install
// time doesn't break `npm install`.

const { ensureBinary } = require('../lib/download');

if (process.env.SMOLSONIC_SKIP_DOWNLOAD) {
  process.stderr.write('smolsonic: SMOLSONIC_SKIP_DOWNLOAD set, skipping binary download\n');
  process.exit(0);
}

ensureBinary().catch((err) => {
  process.stderr.write(
    `smolsonic: could not download binary during install (${err && err.message ? err.message : err}).\n` +
      `smolsonic: it will be fetched automatically the first time you run \`smolsonic\`.\n`
  );
  process.exit(0);
});
