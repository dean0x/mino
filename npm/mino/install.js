#!/usr/bin/env node

/**
 * Postinstall fallback script for @dean0x/mino
 *
 * This script runs after npm install. If the platform-specific optional
 * dependency was installed successfully, this script does nothing.
 * Otherwise, it downloads the correct binary from the npm registry.
 */

const https = require("https");
const fs = require("fs");
const path = require("path");
const zlib = require("zlib");
const { execSync } = require("child_process");

const BINARY_NAME = "mino";
const PACKAGE_VERSION = require("./package.json").version;

/**
 * Platform-specific package mapping
 */
const PLATFORM_PACKAGES = {
  "darwin-x64": "@dean0x/mino-darwin-x64",
  "darwin-arm64": "@dean0x/mino-darwin-arm64",
  "linux-x64": "@dean0x/mino-linux-x64",
  "linux-arm64": "@dean0x/mino-linux-arm64",
};

/**
 * Get the platform key for the current system
 * @returns {string | null}
 */
function getPlatformKey() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === "darwin" && arch === "x64") return "darwin-x64";
  if (platform === "darwin" && arch === "arm64") return "darwin-arm64";
  if (platform === "linux" && arch === "x64") return "linux-x64";
  if (platform === "linux" && arch === "arm64") return "linux-arm64";

  return null;
}

/**
 * Check if binary is already available from optional dependencies
 * @returns {boolean}
 */
function binaryAlreadyAvailable() {
  const platformKey = getPlatformKey();
  if (!platformKey) return false;

  const packageName = PLATFORM_PACKAGES[platformKey];
  if (!packageName) return false;

  try {
    const packagePath = require.resolve(`${packageName}/package.json`);
    const packageDir = path.dirname(packagePath);
    const binaryPath = path.join(packageDir, "bin", BINARY_NAME);
    return fs.existsSync(binaryPath);
  } catch {
    return false;
  }
}

/**
 * Download file from URL with redirect handling
 * @param {string} url
 * @returns {Promise<Buffer>}
 */
function download(url) {
  return new Promise((resolve, reject) => {
    const request = https.get(url, (response) => {
      // Handle redirects
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        download(response.headers.location).then(resolve).catch(reject);
        return;
      }

      if (response.statusCode !== 200) {
        reject(new Error(`HTTP ${response.statusCode}: ${url}`));
        return;
      }

      const chunks = [];
      response.on("data", (chunk) => chunks.push(chunk));
      response.on("end", () => resolve(Buffer.concat(chunks)));
      response.on("error", reject);
    });

    request.on("error", reject);
    request.setTimeout(60000, () => {
      request.destroy();
      reject(new Error("Download timeout"));
    });
  });
}

/**
 * Extract tarball and find the binary
 * @param {Buffer} tarballBuffer
 * @returns {Buffer}
 */
function extractBinaryFromTarball(tarballBuffer) {
  // Decompress gzip
  const tarBuffer = zlib.gunzipSync(tarballBuffer);

  // Simple tar extraction - find the binary file
  // tar format: 512-byte headers followed by file content
  let offset = 0;
  while (offset < tarBuffer.length) {
    // Read header
    const header = tarBuffer.subarray(offset, offset + 512);

    // Check for end of archive (empty header)
    if (header.every((b) => b === 0)) break;

    // Extract filename (first 100 bytes, null-terminated)
    let nameEnd = 0;
    while (nameEnd < 100 && header[nameEnd] !== 0) nameEnd++;
    const name = header.subarray(0, nameEnd).toString("utf8");

    // Extract file size (octal string at offset 124, 12 bytes)
    const sizeStr = header.subarray(124, 136).toString("utf8").trim();
    const size = parseInt(sizeStr, 8) || 0;

    offset += 512; // Move past header

    // Check if this is the binary we're looking for
    if (name.endsWith(`/bin/${BINARY_NAME}`) || name === `bin/${BINARY_NAME}`) {
      return tarBuffer.subarray(offset, offset + size);
    }

    // Skip to next file (rounded up to 512-byte boundary)
    offset += Math.ceil(size / 512) * 512;
  }

  throw new Error(`Binary not found in tarball`);
}

/**
 * Download and install the binary as fallback
 */
async function installFallback() {
  const platformKey = getPlatformKey();
  if (!platformKey) {
    console.log(`Unsupported platform: ${process.platform}-${process.arch}`);
    console.log("Skipping postinstall - binary must be installed manually.");
    return;
  }

  const packageName = PLATFORM_PACKAGES[platformKey];

  console.log(`Downloading ${packageName}@${PACKAGE_VERSION}...`);

  // Get package metadata from npm registry
  const registryUrl = `https://registry.npmjs.org/${packageName}/${PACKAGE_VERSION}`;
  const metadataBuffer = await download(registryUrl);
  const metadata = JSON.parse(metadataBuffer.toString("utf8"));

  const tarballUrl = metadata.dist.tarball;
  if (!tarballUrl) {
    throw new Error("Could not find tarball URL in package metadata");
  }

  // Download tarball
  console.log("Extracting binary...");
  const tarballBuffer = await download(tarballUrl);

  // Extract binary
  const binaryBuffer = extractBinaryFromTarball(tarballBuffer);

  // Write binary to local directory
  const binaryDir = path.join(__dirname, ".binary");
  const binaryPath = path.join(binaryDir, BINARY_NAME);

  fs.mkdirSync(binaryDir, { recursive: true });
  fs.writeFileSync(binaryPath, binaryBuffer, { mode: 0o755 });

  console.log(`Installed ${BINARY_NAME} to ${binaryPath}`);
}

/**
 * Main entry point
 */
async function main() {
  // Skip if running in CI or if explicitly disabled
  if (process.env.MINO_SKIP_INSTALL) {
    console.log("MINO_SKIP_INSTALL is set, skipping postinstall");
    return;
  }

  // Skip if binary already available from optional dependencies
  if (binaryAlreadyAvailable()) {
    console.log("Binary available from optional dependency, skipping fallback download");
    return;
  }

  try {
    await installFallback();
  } catch (error) {
    // Don't fail the install - npm will have already printed warnings about
    // optional dependencies. The user can still install manually.
    console.error(`Warning: Failed to download fallback binary: ${error.message}`);
    console.error("You may need to install the binary manually.");
  }
}

main().catch((error) => {
  console.error(error);
  // Exit 0 so npm install doesn't fail
  process.exit(0);
});
