import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = process.cwd();
// libarchive.a lives in the engine tree. Honor IOS_LINUXKIT_DIR so the final
// Xcode link uses the same engine checkout as build.rs / the linuxkit build,
// falling back to the sibling ../ios-linuxkit checkout (relative to PROJECT_DIR)
// when the env var is unset.
const libarchivePath = process.env.IOS_LINUXKIT_DIR
  ? `${process.env.IOS_LINUXKIT_DIR}/deps/build/Release-iphoneos/libarchive.a`
  : "$(PROJECT_DIR)/../../../../ios-linuxkit/deps/build/Release-iphoneos/libarchive.a";
// AVFoundation is needed by native/ios_keepalive.mm (background audio keep-alive).
// The final executable is linked by xcode, which does not honor cargo's
// rustc-link-lib=framework directives, so add it to the xcode link explicitly.
const ldflags = `$(inherited) -lsqlite3 -lz -lbz2 -liconv -lresolv -framework AVFoundation ${libarchivePath}`;

// The app bundle identifier, read from tauri.conf.json so the pbxproj anchors
// below are not hardcoded (a renamed dev variant, e.g. com.dcmmc.teraxdbg, must
// still be patched or the OTHER_LDFLAGS above are silently dropped → link errors).
const bundleId = (() => {
  try {
    const conf = JSON.parse(
      readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"),
    );
    return conf.identifier || "io.carmo.terax";
  } catch {
    return "io.carmo.terax";
  }
})();

// Signing. Default: DISABLED — `tauri ios build` produces an UNSIGNED .app that
// is fake-signed afterwards with ldid (jailbreak / sideload). The IPA export step
// fails without signing — expected and ignored; we package the .app ourselves.
// Opt-in: set TERAX_IOS_DEV_TEAM=<teamID> to enable automatic Apple signing for a
// dev-signed build installable over USB (devicectl / ideviceinstaller, no AirDrop).
const devTeam = process.env.TERAX_IOS_DEV_TEAM;
const signingSettings = devTeam
  ? {
      CODE_SIGNING_ALLOWED: "YES",
      CODE_SIGNING_REQUIRED: "YES",
      CODE_SIGN_STYLE: "Automatic",
      DEVELOPMENT_TEAM: devTeam,
    }
  : {
      CODE_SIGNING_ALLOWED: "NO",
      CODE_SIGNING_REQUIRED: "NO",
      CODE_SIGN_IDENTITY: "",
      CODE_SIGN_ENTITLEMENTS: "",
      DEVELOPMENT_TEAM: "",
    };

const projectYmlPath = join(root, "src-tauri/gen/apple/project.yml");
const pbxPath = join(root, "src-tauri/gen/apple/terax.xcodeproj/project.pbxproj");

if (existsSync(projectYmlPath)) {
  const original = readFileSync(projectYmlPath, "utf8");
  let next = original;
  if (next.includes("OTHER_LDFLAGS:")) {
    next = next.replace(
      /^(\s+)OTHER_LDFLAGS:.*(?:\n\1OTHER_LDFLAGS:.*)*/m,
      `        OTHER_LDFLAGS: ${ldflags}`,
    );
  } else if (!next.includes(`OTHER_LDFLAGS: ${ldflags}`)) {
    next = next.replace(
      /(\n\s+EXCLUDED_ARCHS\[sdk=iphoneos\*\]: x86_64)/,
      `$1\n        OTHER_LDFLAGS: ${ldflags}`,
    );
  }
  // Inject signing-off keys into the same settings block (anchored on the
  // EXCLUDED_ARCHS line). xcodegen quotes empty strings as "".
  for (const [key, value] of Object.entries(signingSettings)) {
    const yamlValue = value === "" ? '""' : `"${value}"`;
    if (new RegExp(`\\n\\s+${key}:`).test(next)) {
      next = next.replace(new RegExp(`(\\n(\\s+)${key}:).*`), `$1 ${yamlValue}`);
    } else {
      next = next.replace(
        /(\n\s+EXCLUDED_ARCHS\[sdk=iphoneos\*\]: x86_64)/,
        `$1\n        ${key}: ${yamlValue}`,
      );
    }
  }
  if (next !== original) {
    writeFileSync(projectYmlPath, next);
  }
}

if (!existsSync(pbxPath)) {
  process.exit(0);
}

// Set a `KEY = VALUE;` build setting inside a pbxproj buildSettings block,
// replacing an existing entry or inserting one before PRODUCT_BUNDLE_IDENTIFIER.
function setPbxSetting(block, key, rawValue) {
  const value = rawValue === "" ? '""' : rawValue;
  const assignment = `${key} = ${value};`;
  const existing = new RegExp(`${key} = [^;]*;`);
  if (existing.test(block)) {
    return block.replace(existing, assignment);
  }
  const anchor = new RegExp(
    `(\\n\\t\\t\\t\\tPRODUCT_BUNDLE_IDENTIFIER = ${bundleId.replace(/\./g, "\\.")};)`,
  );
  return block.replace(anchor, `\n\t\t\t\t${assignment}$1`);
}

const original = readFileSync(pbxPath, "utf8");
let patchedBlocks = 0;
const next = original.replace(/buildSettings = \{[\s\S]*?\n\t\t\t\};/g, (block) => {
  if (!block.includes(`PRODUCT_BUNDLE_IDENTIFIER = ${bundleId};`)) {
    return block;
  }
  patchedBlocks += 1;
  let patched = setPbxSetting(block, "OTHER_LDFLAGS", `"${ldflags}"`);
  for (const [key, value] of Object.entries(signingSettings)) {
    patched = setPbxSetting(patched, key, value);
  }
  return patched;
});

if (next !== original) {
  writeFileSync(pbxPath, next);
}

if (patchedBlocks === 0) {
  console.warn("No Terax iOS target build settings found to patch.");
}

// Add UIBackgroundModes=[audio] to the iOS Info.plist so the LinuxKit engine
// (and the debug bridge) keeps running when the app isn't foregrounded. The
// active silent audio session is started natively (native/ios_keepalive.mm);
// without this plist key iOS suspends the app after ~30s. xcodegen already ran
// during `tauri ios init`, so we patch the generated Info.plist directly.
const infoPlistPath = join(root, "src-tauri/gen/apple/terax_iOS/Info.plist");
if (existsSync(infoPlistPath)) {
  const plist = readFileSync(infoPlistPath, "utf8");
  if (!plist.includes("UIBackgroundModes")) {
    const entry =
      "\t<key>UIBackgroundModes</key>\n\t<array>\n\t\t<string>audio</string>\n\t</array>\n";
    const patched = plist.replace(/\n<\/dict>\n<\/plist>/, `\n${entry}</dict>\n</plist>`);
    if (patched !== plist) {
      writeFileSync(infoPlistPath, patched);
      console.log("Patched Info.plist: added UIBackgroundModes=[audio]");
    } else {
      console.warn("Could not locate </dict></plist> anchor in Info.plist");
    }
  }
}
