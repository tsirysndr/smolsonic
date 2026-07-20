'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const https = require('https');
const crypto = require('crypto');
const { spawnSync } = require('child_process');

const { REPO, resolveTarget, binaryPath } = require('./platform');

const VERSION = require('../package.json').version;

function assetName(version, label) {
  return `smolsonic-v${version}-${label}.tar.gz`;
}

function assetUrl(version, label) {
  return `https://github.com/${REPO}/releases/download/v${version}/${assetName(version, label)}`;
}

// GET a URL into a Buffer, following redirects (GitHub release downloads
// redirect to a signed object storage URL).
function fetchBuffer(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 10) return reject(new Error(`too many redirects for ${url}`));
    const req = https.get(
      url,
      { headers: { 'User-Agent': 'smolsonic-npm-installer' } },
      (res) => {
        const { statusCode, headers } = res;
        if (statusCode >= 300 && statusCode < 400 && headers.location) {
          res.resume();
          const next = new URL(headers.location, url).toString();
          return resolve(fetchBuffer(next, redirects + 1));
        }
        if (statusCode !== 200) {
          res.resume();
          return reject(new Error(`GET ${url} failed with HTTP ${statusCode}`));
        }
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => resolve(Buffer.concat(chunks)));
        res.on('error', reject);
      }
    );
    req.on('error', reject);
  });
}

function sha256(buf) {
  return crypto.createHash('sha256').update(buf).digest('hex');
}

// Best-effort checksum verification. Returns silently if the .sha256 asset
// can't be fetched; throws only on an actual mismatch.
async function verifyChecksum(tarball, version, label) {
  let expected;
  try {
    const raw = (await fetchBuffer(`${assetUrl(version, label)}.sha256`)).toString('utf8');
    expected = raw.trim().split(/\s+/)[0];
  } catch (_) {
    return; // checksum unavailable — skip
  }
  if (!expected) return;
  const actual = sha256(tarball);
  if (actual !== expected) {
    throw new Error(
      `checksum mismatch for ${assetName(version, label)}\n  expected ${expected}\n  actual   ${actual}`
    );
  }
}

// Extract the `smolsonic` member from a .tar.gz buffer and return the path to
// the extracted binary inside a temp dir. Relies on the system `tar`, which is
// present on all supported (Unix) targets.
function extractBinary(tarball) {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'smolsonic-'));
  const tarPath = path.join(tmpDir, 'smolsonic.tar.gz');
  fs.writeFileSync(tarPath, tarball);
  const res = spawnSync('tar', ['-xzf', tarPath, '-C', tmpDir], { stdio: 'inherit' });
  if (res.error) throw res.error;
  if (res.status !== 0) throw new Error(`tar exited with status ${res.status}`);
  const extracted = path.join(tmpDir, 'smolsonic');
  if (!fs.existsSync(extracted)) {
    throw new Error(`archive did not contain a smolsonic binary`);
  }
  return { extracted, tmpDir };
}

// Download, verify, and install the binary for this platform if not already
// present. Idempotent. Set SMOLSONIC_SKIP_DOWNLOAD=1 to no-op (e.g. in CI).
async function ensureBinary({ force = false, version = VERSION } = {}) {
  const dest = binaryPath();
  if (!force && fs.existsSync(dest)) return dest;

  const { key, label } = resolveTarget();
  if (!label) {
    throw new Error(
      `smolsonic has no prebuilt binary for your platform (${key}).\n` +
        `Supported: darwin/linux/freebsd/netbsd (x64, arm64) and openbsd (x64).\n` +
        `Build from source instead: https://github.com/${REPO}`
    );
  }

  const url = assetUrl(version, label);
  process.stderr.write(`smolsonic: downloading ${assetName(version, label)} …\n`);
  const tarball = await fetchBuffer(url);
  await verifyChecksum(tarball, version, label);

  const { extracted, tmpDir } = extractBinary(tarball);
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(extracted, dest);
  fs.chmodSync(dest, 0o755);
  fs.rmSync(tmpDir, { recursive: true, force: true });

  process.stderr.write(`smolsonic: installed ${label} binary\n`);
  return dest;
}

module.exports = { ensureBinary, assetName, assetUrl, VERSION };
