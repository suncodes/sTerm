import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { checkVersion, syncVersion } from "./version.mjs";

function writeFixture(root, file, content) {
  writeFileSync(join(root, file), content, "utf8");
}

function createFixture() {
  const root = mkdtempSync(join(tmpdir(), "sterm-version-"));
  mkdirSync(join(root, "src-tauri"), { recursive: true });
  writeFixture(root, "package.json", '{\n  "name": "sterm",\n  "version": "0.1.0"\n}\n');
  writeFixture(
    root,
    "package-lock.json",
    '{\n  "name": "sterm",\n  "version": "0.1.0",\n  "lockfileVersion": 3,\n  "packages": {\n    "": {\n      "name": "sterm",\n      "version": "0.1.0"\n    }\n  }\n}\n',
  );
  writeFixture(root, "src-tauri/tauri.conf.json", '{\n  "version": "0.1.0"\n}\n');
  writeFixture(root, "src-tauri/Cargo.toml", '[package]\nname = "sterm"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\ntauri = "2"\n');
  writeFixture(
    root,
    "src-tauri/Cargo.lock",
    'version = 4\n\n[[package]]\nname = "other"\nversion = "9.9.9"\n\n[[package]]\nname = "sterm"\nversion = "0.1.0"\n',
  );
  return root;
}

test("syncVersion synchronizes all derived metadata", (t) => {
  const root = createFixture();
  t.after(() => rmSync(root, { recursive: true, force: true }));

  assert.equal(syncVersion(root, "v1.2.3"), "1.2.3");
  assert.equal(checkVersion(root), "1.2.3");

  const packageJson = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const packageLock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
  const tauriConfig = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));
  assert.equal(packageJson.version, "1.2.3");
  assert.equal(packageLock.version, "1.2.3");
  assert.equal(packageLock.packages[""].version, "1.2.3");
  assert.equal(tauriConfig.version, "../package.json");
  assert.match(readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8"), /version = "1\.2\.3"/);
  assert.match(readFileSync(join(root, "src-tauri/Cargo.lock"), "utf8"), /name = "sterm"\nversion = "1\.2\.3"/);
});

test("checkVersion reports unsynchronized metadata", (t) => {
  const root = createFixture();
  t.after(() => rmSync(root, { recursive: true, force: true }));

  syncVersion(root, "1.2.3");
  writeFixture(
    root,
    "src-tauri/Cargo.toml",
    readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8").replace('version = "1.2.3"', 'version = "1.2.4"'),
  );

  assert.throws(() => checkVersion(root), /版本元数据未同步/);
});
