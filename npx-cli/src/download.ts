import https from "https";
import fs from "fs";
import path from "path";
import crypto from "crypto";
import os from "os";

export const BINARY_TAG = "__BINARY_TAG__"; // e.g., v0.0.135-20251215122030

// Replaced during npm pack by the prerelease workflow with the exact GitHub
// release asset directory for BINARY_TAG. Runtime override must also be the
// exact directory containing manifest.json and the flat binary zip assets.
const DEFAULT_RELEASE_ASSET_BASE_URL = "__GITHUB_RELEASE_ASSET_BASE_URL__";
export const RELEASE_ASSET_BASE_URL = normalizeReleaseAssetBaseUrl(
  process.env.CDESKTOP_RELEASE_ASSET_BASE_URL || DEFAULT_RELEASE_ASSET_BASE_URL,
);
export const CACHE_DIR = path.join(os.homedir(), ".cdesktop", "bin");

// Local development mode: use binaries from npx-cli/dist/ instead of a release
// Only activate if dist/ exists (i.e., running from source after local-build.sh)
export const LOCAL_DIST_DIR = path.join(__dirname, "..", "dist");
export const LOCAL_DEV_MODE =
  fs.existsSync(LOCAL_DIST_DIR) || process.env.CDESKTOP_LOCAL === "1";

export interface BinaryInfo {
  sha256: string;
  size: number;
}

export interface BinaryManifest {
  version: string;
  assets: Record<string, BinaryInfo>;
  platforms: Record<string, Record<string, BinaryInfo>>;
}

export interface DesktopPlatformInfo {
  file: string;
  sha256: string;
  type: string | null;
}

export interface DesktopManifest {
  platforms: Record<string, DesktopPlatformInfo>;
}

export interface DesktopBundleInfo {
  archivePath: string | null;
  dir: string;
  type: string | null;
}

type ProgressCallback = (downloaded: number, total: number) => void;

export function normalizeReleaseAssetBaseUrl(url: string): string {
  return url.replace(/\/+$/, "");
}

export function releaseAssetName(binaryName: string, platform: string): string {
  return `${binaryName}-${platform}.zip`;
}

export function releaseAssetUrl(baseUrl: string, assetName: string): string {
  return `${normalizeReleaseAssetBaseUrl(baseUrl)}/${assetName}`;
}

export function binaryInfoForAsset(
  manifest: BinaryManifest,
  platform: string,
  binaryName: string,
): BinaryInfo | undefined {
  const assetName = releaseAssetName(binaryName, platform);
  return (
    manifest.assets?.[assetName] ?? manifest.platforms?.[platform]?.[binaryName]
  );
}

function fetchJson<T>(url: string): Promise<T> {
  return new Promise((resolve, reject) => {
    https
      .get(url, (res) => {
        if (res.statusCode === 301 || res.statusCode === 302) {
          return fetchJson<T>(res.headers.location!)
            .then(resolve)
            .catch(reject);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode} fetching ${url}`));
        }
        let data = "";
        res.on("data", (chunk: string) => (data += chunk));
        res.on("end", () => {
          try {
            resolve(JSON.parse(data) as T);
          } catch {
            reject(new Error(`Failed to parse JSON from ${url}`));
          }
        });
      })
      .on("error", reject);
  });
}

export function validateDownloadedFile(
  filePath: string,
  expected: BinaryInfo,
): void {
  const data = fs.readFileSync(filePath);
  const actualSha256 = crypto.createHash("sha256").update(data).digest("hex");

  if (expected.size > 0 && data.length !== expected.size) {
    throw new Error(
      `Size mismatch: expected ${expected.size}, got ${data.length}`,
    );
  }

  if (actualSha256 !== expected.sha256) {
    throw new Error(
      `Checksum mismatch: expected ${expected.sha256}, got ${actualSha256}`,
    );
  }
}

function downloadFile(
  url: string,
  destPath: string,
  expected: BinaryInfo,
  onProgress?: ProgressCallback,
): Promise<string> {
  const tempPath = destPath + ".tmp";
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(tempPath);
    const hash = crypto.createHash("sha256");

    const cleanup = () => {
      try {
        fs.unlinkSync(tempPath);
      } catch {}
    };

    https
      .get(url, (res) => {
        if (res.statusCode === 301 || res.statusCode === 302) {
          file.close();
          cleanup();
          return downloadFile(
            res.headers.location!,
            destPath,
            expected,
            onProgress,
          )
            .then(resolve)
            .catch(reject);
        }

        if (res.statusCode !== 200) {
          file.close();
          cleanup();
          return reject(new Error(`HTTP ${res.statusCode} downloading ${url}`));
        }

        const totalSize = parseInt(res.headers["content-length"] || "0", 10);
        let downloadedSize = 0;

        res.on("data", (chunk: Buffer) => {
          downloadedSize += chunk.length;
          hash.update(chunk);
          if (onProgress) onProgress(downloadedSize, totalSize);
        });
        res.pipe(file);

        file.on("finish", () => {
          file.close();
          const actualSha256 = hash.digest("hex");
          const actualSize = fs.statSync(tempPath).size;
          if (expected.size > 0 && actualSize !== expected.size) {
            cleanup();
            reject(
              new Error(
                `Size mismatch: expected ${expected.size}, got ${actualSize}`,
              ),
            );
          } else if (actualSha256 !== expected.sha256) {
            cleanup();
            reject(
              new Error(
                `Checksum mismatch: expected ${expected.sha256}, got ${actualSha256}`,
              ),
            );
          } else {
            try {
              fs.renameSync(tempPath, destPath);
              resolve(destPath);
            } catch (err) {
              cleanup();
              reject(err);
            }
          }
        });
      })
      .on("error", (err) => {
        file.close();
        cleanup();
        reject(err);
      });
  });
}

export async function ensureBinary(
  platform: string,
  binaryName: string,
  onProgress?: ProgressCallback,
): Promise<string> {
  // In local dev mode, use binaries directly from npx-cli/dist/
  if (LOCAL_DEV_MODE) {
    const localZipPath = path.join(
      LOCAL_DIST_DIR,
      platform,
      `${binaryName}.zip`,
    );
    if (fs.existsSync(localZipPath)) {
      return localZipPath;
    }
    throw new Error(
      `Local binary not found: ${localZipPath}\n` +
        `Run ./local-build.sh first to build the binaries.`,
    );
  }

  const cacheDir = path.join(CACHE_DIR, BINARY_TAG, platform);
  const zipPath = path.join(cacheDir, `${binaryName}.zip`);

  fs.mkdirSync(cacheDir, { recursive: true });

  const manifest = await fetchJson<BinaryManifest>(
    releaseAssetUrl(RELEASE_ASSET_BASE_URL, "manifest.json"),
  );
  const binaryInfo = binaryInfoForAsset(manifest, platform, binaryName);

  if (!binaryInfo) {
    throw new Error(`Binary ${binaryName} not available for ${platform}`);
  }

  if (fs.existsSync(zipPath)) {
    try {
      validateDownloadedFile(zipPath, binaryInfo);
      return zipPath;
    } catch {
      fs.unlinkSync(zipPath);
    }
  }

  const assetName = releaseAssetName(binaryName, platform);
  const url = releaseAssetUrl(RELEASE_ASSET_BASE_URL, assetName);
  await downloadFile(url, zipPath, binaryInfo, onProgress);

  return zipPath;
}

export const DESKTOP_CACHE_DIR = path.join(
  os.homedir(),
  ".cdesktop",
  "desktop",
);

export async function ensureDesktopBundle(
  tauriPlatform: string,
  onProgress?: ProgressCallback,
): Promise<DesktopBundleInfo> {
  // In local dev mode, use Tauri bundle from npx-cli/dist/tauri/<platform>/
  if (LOCAL_DEV_MODE) {
    const localDir = path.join(LOCAL_DIST_DIR, "tauri", tauriPlatform);
    if (fs.existsSync(localDir)) {
      const files = fs.readdirSync(localDir);
      const archive = files.find(
        (f) => f.endsWith(".tar.gz") || f.endsWith("-setup.exe"),
      );
      return {
        dir: localDir,
        archivePath: archive ? path.join(localDir, archive) : null,
        type: null,
      };
    }
    throw new Error(
      `Local desktop bundle not found: ${localDir}\n` +
        `Run './local-build.sh --desktop' first to build the Tauri app.`,
    );
  }

  const cacheDir = path.join(DESKTOP_CACHE_DIR, BINARY_TAG, tauriPlatform);

  // Check if already installed (sentinel file from previous run)
  const sentinelPath = path.join(cacheDir, ".installed");
  if (fs.existsSync(sentinelPath)) {
    return { dir: cacheDir, archivePath: null, type: null };
  }

  fs.mkdirSync(cacheDir, { recursive: true });

  // Fetch the desktop manifest
  const manifest = await fetchJson<DesktopManifest>(
    `${RELEASE_ASSET_BASE_URL}/tauri/desktop-manifest.json`,
  );
  const platformInfo = manifest.platforms?.[tauriPlatform];
  if (!platformInfo) {
    throw new Error(`Desktop app not available for platform: ${tauriPlatform}`);
  }

  const destPath = path.join(cacheDir, platformInfo.file);

  // Skip download if file already exists (e.g. previous failed install)
  if (!fs.existsSync(destPath)) {
    const url = `${RELEASE_ASSET_BASE_URL}/tauri/${tauriPlatform}/${platformInfo.file}`;
    await downloadFile(
      url,
      destPath,
      { sha256: platformInfo.sha256, size: 0 },
      onProgress,
    );
  }

  return {
    archivePath: destPath,
    dir: cacheDir,
    type: platformInfo.type,
  };
}

export async function getLatestVersion(): Promise<string | undefined> {
  // The public CLI is pinned to one GitHub release asset directory. There is
  // no mutable "latest binaries" manifest in that distribution path.
  return undefined;
}
