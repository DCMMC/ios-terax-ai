import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = process.cwd();
const ldflags =
  "$(inherited) -lsqlite3 -lz -lbz2 -liconv -lresolv $(PROJECT_DIR)/../../../../ios-linuxkit/deps/build/Release-iphoneos/libarchive.a";

// Disable code signing on the Terax iOS target so `tauri ios build` produces an
// UNSIGNED .app (no Apple certificate / provisioning profile needed). The app is
// fake-signed afterwards with ldid (scripts/ios-fakesign.mjs) for jailbreak /
// sideload installs. The IPA export step of `tauri ios build` will fail without
// signing — that is expected and ignored; we package the unsigned .app ourselves.
const signingSettings = {
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
  return block.replace(
    /(\n\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = io\.carmo\.terax;)/,
    `\n\t\t\t\t${assignment}$1`,
  );
}

const original = readFileSync(pbxPath, "utf8");
let patchedBlocks = 0;
const next = original.replace(/buildSettings = \{[\s\S]*?\n\t\t\t\};/g, (block) => {
  if (!block.includes("PRODUCT_BUNDLE_IDENTIFIER = io.carmo.terax;")) {
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
