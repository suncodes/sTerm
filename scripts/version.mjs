import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PROJECT_ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
const TAURI_VERSION_SOURCE = "../package.json";
const IDENTIFIER = "(?:0|[1-9]\\d*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)";
const SEMVER_PATTERN = new RegExp(
  "^(?:v)?(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)" +
    "(?:-" + IDENTIFIER + "(?:\\." + IDENTIFIER + ")*)?" +
    "(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?$",
);

function versionFiles(root) {
  return {
    packageJson: join(root, "package.json"),
    packageLock: join(root, "package-lock.json"),
    cargoToml: join(root, "src-tauri", "Cargo.toml"),
    cargoLock: join(root, "src-tauri", "Cargo.lock"),
    tauriConfig: join(root, "src-tauri", "tauri.conf.json"),
  };
}

function readText(file) {
  return readFileSync(file, "utf8");
}

function writeText(file, content) {
  writeFileSync(file, content, "utf8");
}

function withoutBom(content) {
  return content.startsWith("\uFEFF") ? content.slice(1) : content;
}

function parseJson(content, file) {
  try {
    return JSON.parse(withoutBom(content));
  } catch (reason) {
    throw new Error(file + " 不是合法 JSON：" + reason.message);
  }
}

function serializeJson(value, originalContent) {
  const bom = originalContent.startsWith("\uFEFF") ? "\uFEFF" : "";
  const eol = originalContent.includes("\r\n") ? "\r\n" : "\n";
  const trailingEol = originalContent.endsWith("\r\n") || originalContent.endsWith("\n");
  const serialized = JSON.stringify(value, null, 2).replace(/\n/g, eol);
  return bom + serialized + (trailingEol ? eol : "");
}

export function normalizeVersion(value) {
  if (typeof value !== "string") {
    throw new Error("版本号必须是字符串。");
  }

  const input = value.trim();
  if (!SEMVER_PATTERN.test(input)) {
    throw new Error("版本号必须是 SemVer，例如 0.1.4 或 v0.1.4。");
  }

  return input.startsWith("v") ? input.slice(1) : input;
}

function cargoPackageSection(content, file) {
  const packageHeader = /^[ \t]*\[package\][ \t]*$/m.exec(content);
  if (!packageHeader || packageHeader.index === undefined) {
    throw new Error(file + " 缺少 [package] 配置段。");
  }

  const nextHeaderPattern = /^[ \t]*\[{1,2}[^\r\n]+\]{1,2}[ \t]*$/gm;
  nextHeaderPattern.lastIndex = packageHeader.index + packageHeader[0].length;
  const nextHeader = nextHeaderPattern.exec(content);
  const end = nextHeader?.index ?? content.length;
  return {
    start: packageHeader.index,
    end,
    content: content.slice(packageHeader.index, end),
  };
}

function cargoPackageVersion(content, file) {
  const section = cargoPackageSection(content, file);
  const matches = [...section.content.matchAll(/^[ \t]*version[ \t]*=[ \t]*"([^"]+)"[ \t]*$/gm)];
  if (matches.length !== 1) {
    throw new Error(file + " 的 [package] 段必须恰好包含一个 version。");
  }
  return matches[0][1];
}

function replaceCargoPackageVersion(content, file, version) {
  const section = cargoPackageSection(content, file);
  const matches = [...section.content.matchAll(/^[ \t]*version[ \t]*=[ \t]*"([^"]+)"[ \t]*$/gm)];
  if (matches.length !== 1) {
    throw new Error(file + " 的 [package] 段必须恰好包含一个 version。");
  }

  const updatedSection = section.content.replace(
    /^([ \t]*version[ \t]*=[ \t]*")[^"]+("[ \t]*)$/m,
    (_line, prefix, suffix) => prefix + version + suffix,
  );
  return content.slice(0, section.start) + updatedSection + content.slice(section.end);
}

function cargoLockPackage(content, file) {
  const packageHeaders = [...content.matchAll(/^[ \t]*\[\[package\]\][ \t]*$/gm)];
  const candidates = [];

  for (let index = 0; index < packageHeaders.length; index++) {
    const start = packageHeaders[index].index;
    const end = packageHeaders[index + 1]?.index ?? content.length;
    const section = content.slice(start, end);
    if (/^[ \t]*name[ \t]*=[ \t]*"sterm"[ \t]*$/m.test(section)) {
      candidates.push({ start, end, content: section });
    }
  }

  if (candidates.length !== 1) {
    throw new Error(file + ' 必须恰好包含一个 name = "sterm" 的 package 段。');
  }
  return candidates[0];
}

function cargoLockVersion(content, file) {
  const section = cargoLockPackage(content, file);
  const matches = [...section.content.matchAll(/^[ \t]*version[ \t]*=[ \t]*"([^"]+)"[ \t]*$/gm)];
  if (matches.length !== 1) {
    throw new Error(file + ' 中 sterm package 段必须恰好包含一个 version。');
  }
  return matches[0][1];
}

function replaceCargoLockVersion(content, file, version) {
  const section = cargoLockPackage(content, file);
  const matches = [...section.content.matchAll(/^[ \t]*version[ \t]*=[ \t]*"([^"]+)"[ \t]*$/gm)];
  if (matches.length !== 1) {
    throw new Error(file + ' 中 sterm package 段必须恰好包含一个 version。');
  }

  const updatedSection = section.content.replace(
    /^([ \t]*version[ \t]*=[ \t]*")[^"]+("[ \t]*)$/m,
    (_line, prefix, suffix) => prefix + version + suffix,
  );
  return content.slice(0, section.start) + updatedSection + content.slice(section.end);
}

function packageLockRoot(lock, file) {
  if (!lock.packages || typeof lock.packages !== "object" || !lock.packages[""]) {
    throw new Error(file + ' 缺少 packages[""] 根包元数据。');
  }
  return lock.packages[""];
}

function replaceTauriVersion(content, file) {
  const config = parseJson(content, file);
  if (!Object.hasOwn(config, "version") || typeof config.version !== "string") {
    throw new Error(file + " 缺少字符串类型的 version。");
  }

  const matches = [...content.matchAll(/^[ \t]*"version"[ \t]*:[ \t]*"[^"]+"[ \t]*,?[ \t]*$/gm)];
  if (matches.length !== 1) {
    throw new Error(file + " 必须恰好包含一个 version 字段。");
  }

  return content.replace(
    /^([ \t]*"version"[ \t]*:[ \t]*)"[^"]+"([ \t]*,?[ \t]*)$/m,
    (_line, prefix, suffix) => prefix + '"' + TAURI_VERSION_SOURCE + '"' + suffix,
  );
}

export function checkVersion(root = PROJECT_ROOT) {
  const files = versionFiles(resolve(root));
  const packageJsonSource = readText(files.packageJson);
  const packageLockSource = readText(files.packageLock);
  const tauriConfigSource = readText(files.tauriConfig);
  const cargoToml = readText(files.cargoToml);
  const cargoLock = readText(files.cargoLock);

  const packageJson = parseJson(packageJsonSource, files.packageJson);
  const packageLock = parseJson(packageLockSource, files.packageLock);
  const tauriConfig = parseJson(tauriConfigSource, files.tauriConfig);
  const version = normalizeVersion(packageJson.version);
  const mismatches = [];

  if (packageJson.version !== version) {
    mismatches.push("package.json 的 version 必须不带 v 前缀。");
  }
  if (packageLock.version !== version) {
    mismatches.push("package-lock.json 顶层 version 为 " + String(packageLock.version) + "。");
  }

  const rootPackage = packageLockRoot(packageLock, files.packageLock);
  if (rootPackage.version !== version) {
    mismatches.push('package-lock.json 的 packages[""].version 为 ' + String(rootPackage.version) + "。");
  }
  if (tauriConfig.version !== TAURI_VERSION_SOURCE) {
    mismatches.push(
      "src-tauri/tauri.conf.json 的 version 必须为 " + TAURI_VERSION_SOURCE + "。",
    );
  }

  const tomlVersion = cargoPackageVersion(cargoToml, files.cargoToml);
  if (tomlVersion !== version) {
    mismatches.push("src-tauri/Cargo.toml 的 package.version 为 " + tomlVersion + "。");
  }

  const lockVersion = cargoLockVersion(cargoLock, files.cargoLock);
  if (lockVersion !== version) {
    mismatches.push("src-tauri/Cargo.lock 的 sterm version 为 " + lockVersion + "。");
  }

  if (mismatches.length > 0) {
    throw new Error("版本元数据未同步：\n- " + mismatches.join("\n- "));
  }
  return version;
}

export function syncVersion(root = PROJECT_ROOT, requestedVersion) {
  const files = versionFiles(resolve(root));
  const version = normalizeVersion(requestedVersion);
  const packageJsonSource = readText(files.packageJson);
  const packageLockSource = readText(files.packageLock);
  const tauriConfigSource = readText(files.tauriConfig);
  const cargoTomlSource = readText(files.cargoToml);
  const cargoLockSource = readText(files.cargoLock);
  const packageJson = parseJson(packageJsonSource, files.packageJson);
  const packageLock = parseJson(packageLockSource, files.packageLock);

  packageJson.version = version;
  packageLock.version = version;
  packageLockRoot(packageLock, files.packageLock).version = version;

  const cargoToml = replaceCargoPackageVersion(cargoTomlSource, files.cargoToml, version);
  const cargoLock = replaceCargoLockVersion(cargoLockSource, files.cargoLock, version);
  const tauriConfig = replaceTauriVersion(tauriConfigSource, files.tauriConfig);

  writeText(files.packageJson, serializeJson(packageJson, packageJsonSource));
  writeText(files.packageLock, serializeJson(packageLock, packageLockSource));
  writeText(files.tauriConfig, tauriConfig);
  writeText(files.cargoToml, cargoToml);
  writeText(files.cargoLock, cargoLock);

  return checkVersion(root);
}

function runCli() {
  const [command, inputVersion, ...extra] = process.argv.slice(2);
  if (command === "set" && inputVersion && extra.length === 0) {
    const version = syncVersion(PROJECT_ROOT, inputVersion);
    process.stdout.write("已将版本同步为 " + version + "。\n");
    return;
  }
  if (command === "check" && !inputVersion) {
    const version = checkVersion(PROJECT_ROOT);
    process.stdout.write("版本元数据一致：" + version + "。\n");
    return;
  }

  throw new Error("用法：node scripts/version.mjs <set <version>|check>");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    runCli();
  } catch (reason) {
    process.stderr.write("版本处理失败：" + reason.message + "\n");
    process.exitCode = 1;
  }
}
