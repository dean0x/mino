#!/usr/bin/env node

const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");

const BINARY_NAME = process.platform === "win32" ? "minotaur.exe" : "minotaur";

/**
 * Platform-specific package mapping
 */
const PLATFORM_PACKAGES = {
  "darwin-x64": "@dean0x/minotaur-darwin-x64",
  "darwin-arm64": "@dean0x/minotaur-darwin-arm64",
  "linux-x64": "@dean0x/minotaur-linux-x64",
  "linux-arm64": "@dean0x/minotaur-linux-arm64",
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
 * Try to resolve binary from optional dependencies
 * @returns {string | null}
 */
function resolveBinaryFromOptionalDeps() {
  const platformKey = getPlatformKey();
  if (!platformKey) return null;

  const packageName = PLATFORM_PACKAGES[platformKey];
  if (!packageName) return null;

  try {
    const packagePath = require.resolve(`${packageName}/package.json`);
    const packageDir = path.dirname(packagePath);
    const binaryPath = path.join(packageDir, "bin", BINARY_NAME);

    if (fs.existsSync(binaryPath)) {
      return binaryPath;
    }
  } catch {
    // Package not installed (expected on non-matching platforms)
  }

  return null;
}

/**
 * Try to resolve binary from postinstall fallback location
 * @returns {string | null}
 */
function resolveBinaryFromFallback() {
  const fallbackPath = path.join(__dirname, "..", ".binary", BINARY_NAME);
  if (fs.existsSync(fallbackPath)) {
    return fallbackPath;
  }
  return null;
}

/**
 * Find the minotaur binary
 * @returns {string}
 */
function findBinary() {
  // Try optional dependencies first (fastest path)
  const optionalDepsBinary = resolveBinaryFromOptionalDeps();
  if (optionalDepsBinary) {
    return optionalDepsBinary;
  }

  // Try postinstall fallback location
  const fallbackBinary = resolveBinaryFromFallback();
  if (fallbackBinary) {
    return fallbackBinary;
  }

  // No binary found
  const platformKey = getPlatformKey();
  if (!platformKey) {
    console.error(
      `Error: Unsupported platform: ${process.platform}-${process.arch}`
    );
    console.error("Minotaur supports: darwin-x64, darwin-arm64, linux-x64, linux-arm64");
    process.exit(1);
  }

  console.error("Error: Could not find minotaur binary.");
  console.error("");
  console.error("This usually means:");
  console.error("  1. npm failed to install the platform-specific package");
  console.error("  2. The postinstall script failed to download the binary");
  console.error("");
  console.error("Try reinstalling: npm install -g @dean0x/minotaur");
  process.exit(1);
}

/**
 * Main entry point
 */
function main() {
  const binaryPath = findBinary();
  const args = process.argv.slice(2);

  const child = spawn(binaryPath, args, {
    stdio: "inherit",
    env: process.env,
  });

  child.on("error", (err) => {
    console.error(`Failed to execute minotaur: ${err.message}`);
    process.exit(1);
  });

  child.on("close", (code) => {
    process.exit(code ?? 0);
  });
}

main();
