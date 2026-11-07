#!/usr/bin/env node
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const paths = {
  package: join(root, "package.json"),
  tauri: join(root, "src-tauri", "tauri.conf.json"),
  cargo: join(root, "src-tauri", "Cargo.toml"),
  lock: join(root, "src-tauri", "Cargo.lock"),
  packageLock: join(root, "package-lock.json"),
};

const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
const CARGO_PACKAGE = /^\[package\][^\n]*\n([\s\S]*?)(?=^\[)/m;
const CARGO_LOCK_QUICKLET =
  /(\[\[package\]\]\r?\nname = "quicklet"\r?\nversion = ")([^"]+)(")/;

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

function readCargoVersion(text) {
  const section = text.match(CARGO_PACKAGE)?.[1];
  if (section === undefined) throw new Error("Cargo.toml is missing [package]");
  const version = section.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (version === undefined) throw new Error("Cargo.toml is missing package.version");
  return version;
}

function writeCargoVersion(text, version) {
  const match = text.match(CARGO_PACKAGE);
  if (match === null) throw new Error("Cargo.toml is missing [package]");
  const section = match[1];
  if (!/^version\s*=\s*"[^"]+"/m.test(section)) {
    throw new Error("Cargo.toml is missing package.version");
  }
  const updatedSection = section.replace(
    /^version\s*=\s*"[^"]+"/m,
    `version = "${version}"`,
  );
  return `${text.slice(0, match.index)}${match[0].replace(section, () => updatedSection)}${text.slice(
    match.index + match[0].length,
  )}`;
}

function readLockVersion(text) {
  return text.match(CARGO_LOCK_QUICKLET)?.[2];
}

function writeLockVersion(text, version) {
  if (!CARGO_LOCK_QUICKLET.test(text)) {
    throw new Error('Cargo.lock is missing the [[package]] name = "quicklet" entry');
  }
  return text.replace(CARGO_LOCK_QUICKLET, `$1${version}$3`);
}

function readBuildNumber(tauri) {
  const buildNumber = tauri.bundle?.macOS?.bundleVersion;
  if (!/^[1-9]\d*$/.test(String(buildNumber ?? ""))) {
    throw new Error(
      'tauri.conf.json bundle.macOS.bundleVersion must be a positive integer',
    );
  }
  return Number(buildNumber);
}

function readPackageLockVersion() {
  if (!existsSync(paths.packageLock)) return undefined;
  const lock = readJson(paths.packageLock);
  const rootVersion = lock.packages?.[""]?.version;
  if (lock.version !== rootVersion) {
    throw new Error("package-lock.json root versions are inconsistent");
  }
  return lock.version;
}

function check() {
  const pkg = readJson(paths.package);
  const tauri = readJson(paths.tauri);
  const cargoText = readFileSync(paths.cargo, "utf8");
  const lockText = readFileSync(paths.lock, "utf8");
  const versions = {
    "package.json": pkg.version,
    "src-tauri/tauri.conf.json": tauri.version,
    "src-tauri/Cargo.toml": readCargoVersion(cargoText),
    "src-tauri/Cargo.lock (quicklet)": readLockVersion(lockText),
  };
  const packageLockVersion = readPackageLockVersion();
  if (packageLockVersion !== undefined) {
    versions["package-lock.json"] = packageLockVersion;
  }

  if (!SEMVER.test(String(pkg.version ?? ""))) {
    throw new Error(`package.json version is not valid SemVer: ${pkg.version}`);
  }

  const mismatches = Object.entries(versions).filter(
    ([, version]) => version !== pkg.version,
  );
  if (mismatches.length > 0) {
    throw new Error(
      `Version mismatch; expected ${pkg.version}:\n${mismatches
        .map(([file, version]) => `  ${file}: ${version ?? "missing"}`)
        .join("\n")}`,
    );
  }

  readBuildNumber(tauri);
  console.log(`Version ${pkg.version} is synchronized (macOS build ${readBuildNumber(tauri)}).`);
}

function setVersion(version) {
  if (!SEMVER.test(String(version ?? ""))) {
    throw new Error(`Usage: npm run version:set -- <semver> (received ${version})`);
  }

  const pkg = readJson(paths.package);
  pkg.version = version;
  writeJson(paths.package, pkg);

  if (existsSync(paths.packageLock)) {
    const packageLock = readJson(paths.packageLock);
    packageLock.version = version;
    packageLock.packages[""].version = version;
    writeJson(paths.packageLock, packageLock);
  }

  const tauri = readJson(paths.tauri);
  tauri.version = version;
  writeJson(paths.tauri, tauri);

  const cargoText = readFileSync(paths.cargo, "utf8");
  writeFileSync(paths.cargo, writeCargoVersion(cargoText, version));

  const lockText = readFileSync(paths.lock, "utf8");
  writeFileSync(paths.lock, writeLockVersion(lockText, version));

  check();
}

function bumpBuild() {
  const tauri = readJson(paths.tauri);
  const next = readBuildNumber(tauri) + 1;
  tauri.bundle.macOS.bundleVersion = String(next);
  writeJson(paths.tauri, tauri);
  check();
}

try {
  const [command, value] = process.argv.slice(2);
  if (command === "check") check();
  else if (command === "set") setVersion(value);
  else if (command === "bump-build") bumpBuild();
  else {
    throw new Error("Usage: version.mjs <check|set <semver>|bump-build>");
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
