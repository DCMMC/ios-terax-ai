// Fake-sign a built iOS .app with ldid and package it into a jailbreak IPA.
//
// This is the certificate-free signing path used for jailbreak / sideload
// installs (AppSync Unified, TrollStore, Filza, etc.). It does NOT need an
// Apple Developer account, provisioning profile, or signing identity.
//
// Pipeline:
//   1. Locate the built `.app` (explicit --app, or newest *.app under --search).
//   2. Recursively find every Mach-O binary inside the bundle (main executable,
//      frameworks, dylibs, app extensions) by inspecting magic bytes.
//   3. ldid fake-sign dependencies first, then the main executable with the
//      jailbreak entitlements plist.
//   4. Package Payload/<App>.app into a .ipa (zip), preserving symlinks.
//
// Usage:
//   bun scripts/ios-fakesign.mjs \
//     --search src-tauri/gen/apple/build/Build/Products \
//     --out src-tauri/gen/apple/build/arm64/Terax.ipa \
//     --entitlements src-tauri/ios/Terax.jailbreak.entitlements
//
//   bun scripts/ios-fakesign.mjs --app /path/to/Terax.app --out /path/to/Terax.ipa

import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { openSync, readSync, closeSync } from "node:fs";
import { spawnSync } from "node:child_process";

const repoRoot = resolve(import.meta.dirname, "..");

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith("--")) continue;
    const name = key.slice(2);
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("--")) {
      args[name] = true;
    } else {
      args[name] = value;
      i += 1;
    }
  }
  return args;
}

function run(command, args) {
  console.log(`$ ${[command, ...args].join(" ")}`);
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.error) {
    console.error(`Failed to spawn ${command}: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`${command} exited with status ${result.status}`);
    process.exit(result.status ?? 1);
  }
}

function requireTool(name, hint) {
  const probe = spawnSync("command", ["-v", name], { shell: true, stdio: "ignore" });
  if (probe.status !== 0) {
    console.error(`Required tool "${name}" not found on PATH.`);
    if (hint) console.error(hint);
    process.exit(1);
  }
}

// Newest *.app anywhere under `root` (skips nested .app inside other .app).
function findApp(root) {
  if (!existsSync(root)) return null;
  const matches = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const full = join(dir, entry.name);
      if (entry.name.endsWith(".app")) {
        matches.push(full);
        continue; // do not descend into the app looking for more top-level apps
      }
      walk(full);
    }
  };
  walk(root);
  if (matches.length === 0) return null;
  matches.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  return matches[0];
}

const MACHO_MAGICS = new Set([
  0xfeedface, // 32-bit
  0xfeedfacf, // 64-bit
  0xcefaedfe, // 32-bit, byte-swapped
  0xcffaedfe, // 64-bit, byte-swapped
  0xcafebabe, // fat / universal
  0xbebafeca, // fat, byte-swapped
]);

function isMachO(file) {
  let fd;
  try {
    fd = openSync(file, "r");
    const buf = Buffer.alloc(4);
    const read = readSync(fd, buf, 0, 4, 0);
    if (read < 4) return false;
    const magic = buf.readUInt32BE(0);
    return MACHO_MAGICS.has(magic);
  } catch {
    return false;
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
}

// Main bundle executable name from Info.plist (CFBundleExecutable).
function mainExecutable(appDir) {
  const info = join(appDir, "Info.plist");
  if (existsSync(info)) {
    const probe = spawnSync(
      "plutil",
      ["-extract", "CFBundleExecutable", "raw", "-o", "-", info],
      { encoding: "utf8" },
    );
    if (probe.status === 0) {
      const name = probe.stdout.trim();
      if (name) return join(appDir, name);
    }
  }
  // Fallback: assume executable matches the bundle name.
  return join(appDir, basename(appDir, ".app"));
}

function collectMachO(appDir) {
  const binaries = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name);
      if (entry.isSymbolicLink()) continue;
      if (entry.isDirectory()) {
        if (entry.name === "_CodeSignature") continue;
        walk(full);
        continue;
      }
      if (entry.isFile() && isMachO(full)) {
        binaries.push(full);
      }
    }
  };
  walk(appDir);
  return binaries;
}

function main() {
  const args = parseArgs(process.argv.slice(2));

  requireTool(
    "ldid",
    "Install it with `brew install ldid` (or use ProcursusTeam/ldid). " +
      "ldid is the standard fake-sign tool for jailbreak IPAs.",
  );

  const searchRoot = args.search
    ? resolve(args.search)
    : resolve(repoRoot, "src-tauri/gen/apple/build/Build/Products");
  const appPath = args.app ? resolve(args.app) : findApp(searchRoot);

  if (!appPath || !existsSync(appPath)) {
    console.error(
      `No .app found. Looked under: ${args.app ?? searchRoot}\n` +
        "Build the unsigned app first (bun run ios:build:jailbreak runs xcodebuild), " +
        "or pass --app <path-to-.app>.",
    );
    process.exit(1);
  }

  const entitlements = resolve(
    args.entitlements ?? join(repoRoot, "src-tauri/ios/Terax.jailbreak.entitlements"),
  );
  if (!existsSync(entitlements)) {
    console.error(`Entitlements plist not found: ${entitlements}`);
    process.exit(1);
  }

  const outIpa = resolve(
    args.out ?? join(dirname(appPath), `${basename(appPath, ".app")}.ipa`),
  );

  console.log(`App:          ${appPath}`);
  console.log(`Entitlements: ${entitlements}`);
  console.log(`Output IPA:   ${outIpa}`);

  const mainExe = mainExecutable(appPath);
  const binaries = collectMachO(appPath);
  const dependencies = binaries.filter((b) => resolve(b) !== resolve(mainExe));

  console.log(
    `Found ${binaries.length} Mach-O binarie(s); signing ${dependencies.length} ` +
      `dependency binarie(s) then the main executable.`,
  );

  // Sign dependencies (frameworks, dylibs, app extensions) without entitlements,
  // then the main executable with the jailbreak entitlements.
  for (const dep of dependencies) {
    run("ldid", ["-S", dep]);
  }
  if (!existsSync(mainExe)) {
    console.error(`Main executable not found inside bundle: ${mainExe}`);
    process.exit(1);
  }
  run("ldid", [`-S${entitlements}`, mainExe]);

  // Package Payload/<App>.app -> .ipa, preserving symlinks.
  const stageDir = join(dirname(outIpa), ".ipa-stage");
  rmSync(stageDir, { recursive: true, force: true });
  const payloadDir = join(stageDir, "Payload");
  mkdirSync(payloadDir, { recursive: true });
  cpSync(appPath, join(payloadDir, basename(appPath)), {
    recursive: true,
    verbatimSymlinks: true,
  });

  rmSync(outIpa, { force: true });
  mkdirSync(dirname(outIpa), { recursive: true });

  // `zip -y` stores symlinks rather than following them (required for frameworks).
  const zip = spawnSync("zip", ["-qry", outIpa, "Payload"], {
    cwd: stageDir,
    stdio: "inherit",
  });
  if (zip.status !== 0) {
    console.error(`zip failed with status ${zip.status}`);
    process.exit(zip.status ?? 1);
  }
  rmSync(stageDir, { recursive: true, force: true });

  console.log(`\nFake-signed jailbreak IPA written to:\n  ${outIpa}`);
  console.log(
    "Install it with TrollStore, or with AppSync Unified via Filza/installer.",
  );
}

main();
