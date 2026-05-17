import { copyFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const repoRoot = resolve(import.meta.dirname, "..");
const iosLinuxKitRoot = resolve(process.env.IOS_LINUXKIT_DIR ?? `${repoRoot}/../ios-linuxkit`);
const deviceId = process.env.IOS_DEVICE_ID ?? "00008103-0015284626DB001E";
const homebrewBin = process.env.TERAX_HOMEBREW_BIN ?? `${repoRoot}/.tmp-homebrew/bin`;
const rootArchive = `${iosLinuxKitRoot}/build/Debug-ApplePleaseFixFB19282108-iphoneos/LinuxKit.app/root.tar.gz`;
const embeddedRootArchive = `${repoRoot}/src-tauri/resources/ios-linuxkit/root.tar.gz`;
const ipaPath = `${repoRoot}/src-tauri/gen/apple/build/arm64/Terax.ipa`;

const env = {
  ...process.env,
  PATH: `${homebrewBin}:${process.env.PATH ?? ""}`,
  GEM_HOME: process.env.GEM_HOME ?? `${repoRoot}/.tmp-gems-ruby4`,
  GEM_PATH: process.env.GEM_PATH ?? `${repoRoot}/.tmp-gems-ruby4`,
};

function run(command, args, options = {}) {
  console.log(`$ ${[command, ...args].join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    env,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function requireFile(path, hint) {
  if (existsSync(path)) return;
  console.error(`Missing ${path}`);
  if (hint) console.error(hint);
  process.exit(1);
}

function buildLinuxKit() {
  run("xcodebuild", [
    "-project",
    `${iosLinuxKitRoot}/iSH.xcodeproj`,
    "-configuration",
    "Debug-ApplePleaseFixFB19282108",
    "-sdk",
    "iphoneos",
    `BUILD_DIR=${iosLinuxKitRoot}/build`,
    "-target",
    "libish",
    "-target",
    "libfakefs",
    "-target",
    "libish_emu",
    "build",
  ]);
}

function syncRoot() {
  requireFile(
    rootArchive,
    "Build the ios-linuxkit app/rootfs first, or set IOS_LINUXKIT_DIR to a repository with build/Debug-ApplePleaseFixFB19282108-iphoneos/LinuxKit.app/root.tar.gz.",
  );
  mkdirSync(dirname(embeddedRootArchive), { recursive: true });
  copyFileSync(rootArchive, embeddedRootArchive);
  console.log(`root.tar.gz ${sha256(embeddedRootArchive)}`);
}

function buildIpa() {
  run("bunx", ["tauri", "ios", "build", "--debug", "--target", "aarch64", "--ci"]);
}

function deploy() {
  requireFile(ipaPath, "Run `bun run ios:build:device` first.");
  run("xcrun", ["devicectl", "device", "install", "app", "--device", deviceId, ipaPath]);
}

function launch() {
  run("xcrun", [
    "devicectl",
    "device",
    "process",
    "launch",
    "--device",
    deviceId,
    "--terminate-existing",
    "--console",
    "io.carmo.terax",
  ]);
}

const commands = {
  "build-linuxkit": buildLinuxKit,
  "sync-root": syncRoot,
  prepare() {
    buildLinuxKit();
    syncRoot();
  },
  build() {
    buildIpa();
  },
  deploy,
  launch,
  test() {
    buildLinuxKit();
    syncRoot();
    buildIpa();
    deploy();
  },
};

const command = process.argv[2] ?? "test";
if (!commands[command]) {
  console.error(`Usage: bun scripts/ios-linuxkit.mjs ${Object.keys(commands).join("|")}`);
  process.exit(2);
}

commands[command]();
