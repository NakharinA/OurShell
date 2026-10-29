// Builds the Tauri updater manifest (latest.json) for a release from the
// signed installers already uploaded to it, after every desktop build job has
// finished. Generating it once here avoids parallel jobs overwriting each other.
//
// Usage: node scripts/make-latest-json.mjs <tag> <dir-with-.sig-files> [notes]
// Writes latest.json to the current directory. Expects the release assets'
// names (and their .sig files) as produced by `tauri build --bundles ...`.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [tag, sigDir, notes = ""] = process.argv.slice(2);
if (!tag || !sigDir) {
  console.error("usage: make-latest-json.mjs <tag> <sig-dir> [notes]");
  process.exit(2);
}
const repo = process.env.GITHUB_REPOSITORY || "NakharinA/OurShell";
const version = tag.replace(/^v/, "");

// Updater artifact per platform (NSIS for Windows, .app.tar.gz for macOS, AppImage for Linux).
const platforms = {
  "windows-x86_64": (n) => /_x64-setup\.exe$/.test(n),
  "windows-aarch64": (n) => /_arm64-setup\.exe$/.test(n),
  "darwin-aarch64": (n) => /aarch64\.app\.tar\.gz$/.test(n),
  "linux-x86_64": (n) => /_amd64\.AppImage$/.test(n),
  "linux-aarch64": (n) => /_aarch64\.AppImage$/.test(n),
};

const sigs = readdirSync(sigDir).filter((f) => f.endsWith(".sig"));
const manifest = { version, notes, pub_date: new Date().toISOString(), platforms: {} };
for (const [platform, matches] of Object.entries(platforms)) {
  const sig = sigs.find((s) => matches(s.slice(0, -4)));
  if (!sig) {
    console.warn(`warning: no signed updater artifact for ${platform}; it will not auto-update`);
    continue;
  }
  const asset = sig.slice(0, -4);
  manifest.platforms[platform] = {
    signature: readFileSync(join(sigDir, sig), "utf8").trim(),
    url: `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(asset)}`,
  };
}
if (Object.keys(manifest.platforms).length === 0) {
  console.error("error: no signed updater artifacts found; is TAURI_SIGNING_PRIVATE_KEY set?");
  process.exit(1);
}
writeFileSync("latest.json", JSON.stringify(manifest, null, 2) + "\n");
console.log(`latest.json: ${Object.keys(manifest.platforms).join(", ")}`);
