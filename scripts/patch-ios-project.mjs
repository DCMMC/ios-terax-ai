import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = process.cwd();
const ldflags = "$(inherited) -lbz2 -liconv";
const projectYmlPath = join(root, "src-tauri/gen/apple/project.yml");
const pbxPath = join(root, "src-tauri/gen/apple/terax.xcodeproj/project.pbxproj");

if (existsSync(projectYmlPath)) {
  const original = readFileSync(projectYmlPath, "utf8");
  let next = original;
  if (!next.includes(`OTHER_LDFLAGS: ${ldflags}`)) {
    next = next.replace(
      /(\n\s+EXCLUDED_ARCHS\[sdk=iphoneos\*\]: x86_64)/,
      `$1\n        OTHER_LDFLAGS: ${ldflags}`,
    );
  }
  if (next !== original) {
    writeFileSync(projectYmlPath, next);
  }
}

if (!existsSync(pbxPath)) {
  process.exit(0);
}

const original = readFileSync(pbxPath, "utf8");
let patchedBlocks = 0;
const next = original.replace(/buildSettings = \{[\s\S]*?\n\t\t\t\};/g, (block) => {
  if (!block.includes("PRODUCT_BUNDLE_IDENTIFIER = io.carmo.terax;")) {
    return block;
  }
  patchedBlocks += 1;
  if (block.includes("OTHER_LDFLAGS")) {
    return block;
  }
  return block.replace(
    /(\n\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = io\.carmo\.terax;)/,
    `\n\t\t\t\tOTHER_LDFLAGS = "${ldflags}";$1`,
  );
});

if (next !== original) {
  writeFileSync(pbxPath, next);
}

if (patchedBlocks === 0) {
  console.warn("No Terax iOS target build settings found to patch.");
}
