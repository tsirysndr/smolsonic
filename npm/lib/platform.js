'use strict';

const path = require('path');

const REPO = 'tsirysndr/smolsonic';

// Maps Node's `${process.platform}-${process.arch}` to the label used in the
// GitHub release asset name: `smolsonic-v<version>-<label>.tar.gz`.
const TARGETS = {
  'darwin-x64': 'macos-amd64',
  'darwin-arm64': 'macos-aarch64',
  'linux-x64': 'linux-amd64',
  'linux-arm64': 'linux-aarch64',
  'freebsd-x64': 'freebsd-amd64',
  'freebsd-arm64': 'freebsd-aarch64',
  'netbsd-x64': 'netbsd-amd64',
  'netbsd-arm64': 'netbsd-aarch64',
  'openbsd-x64': 'openbsd-amd64',
};

function resolveTarget(platform, arch) {
  platform = platform || process.platform;
  arch = arch || process.arch;
  const key = `${platform}-${arch}`;
  return { key, platform, arch, label: TARGETS[key] || null };
}

// Absolute path to the downloaded native binary bundled next to the launcher.
function binaryPath() {
  return path.join(__dirname, '..', 'bin', 'smolsonic-bin');
}

module.exports = { REPO, TARGETS, resolveTarget, binaryPath };
