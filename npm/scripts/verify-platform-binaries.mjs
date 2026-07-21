#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const NPM_ROOT = path.resolve(SCRIPT_DIR, "..");

const TARGETS = [
  ["cli-darwin-arm64", "bin/umadev", "macho", 0x0100000c],
  ["cli-darwin-x64", "bin/umadev", "macho", 0x01000007],
  ["cli-linux-x64", "bin/umadev", "elf", 62, "glibc"],
  ["cli-linux-arm64", "bin/umadev", "elf", 183, "glibc"],
  ["cli-linux-musl-x64", "bin/umadev", "elf", 62, "musl"],
  ["cli-linux-musl-arm64", "bin/umadev", "elf", 183, "musl"],
  ["cli-win32-x64", "bin/umadev.exe", "pe", 0x8664],
];

function fail(message) {
  throw new Error(message);
}

export function verifyBinary(buffer, format, machine, libc, label = "binary") {
  if (buffer.length < 64) fail(`${label}: file is too small to be an executable`);

  if (format === "elf") {
    if (!buffer.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))) {
      fail(`${label}: expected an ELF executable`);
    }
    if (buffer[4] !== 2 || buffer[5] !== 1) {
      fail(`${label}: expected a 64-bit little-endian ELF executable`);
    }
    const actualMachine = buffer.readUInt16LE(18);
    if (actualMachine !== machine) {
      fail(`${label}: ELF machine ${actualMachine} does not match ${machine}`);
    }

    // GNU builds must carry their dynamic loader. Static musl builds do not
    // necessarily mention "musl", so their negative contract is that the GNU
    // loader is absent. This catches the dangerous glibc/musl package swap.
    const text = buffer.toString("latin1");
    const hasGnuLoader = text.includes("ld-linux") || text.includes("ld64.so");
    if (libc === "glibc" && !hasGnuLoader) {
      fail(`${label}: glibc package has no GNU dynamic loader`);
    }
    if (libc === "musl" && hasGnuLoader) {
      fail(`${label}: musl package contains a GNU dynamic loader`);
    }
    return;
  }

  if (format === "macho") {
    if (buffer.readUInt32LE(0) !== 0xfeedfacf) {
      fail(`${label}: expected a 64-bit Mach-O executable`);
    }
    const actualCpu = buffer.readUInt32LE(4);
    if (actualCpu !== machine) {
      fail(`${label}: Mach-O CPU ${actualCpu} does not match ${machine}`);
    }
    return;
  }

  if (format === "pe") {
    if (buffer[0] !== 0x4d || buffer[1] !== 0x5a) {
      fail(`${label}: expected an MZ/PE executable`);
    }
    const peOffset = buffer.readUInt32LE(0x3c);
    if (peOffset + 6 > buffer.length || buffer.toString("ascii", peOffset, peOffset + 4) !== "PE\0\0") {
      fail(`${label}: invalid PE header`);
    }
    const actualMachine = buffer.readUInt16LE(peOffset + 4);
    if (actualMachine !== machine) {
      fail(`${label}: PE machine ${actualMachine} does not match ${machine}`);
    }
    return;
  }

  fail(`${label}: unknown executable format ${format}`);
}

export function verifyPlatformTree(root = NPM_ROOT) {
  for (const [pkg, relative, format, machine, libc] of TARGETS) {
    const file = path.join(root, pkg, relative);
    if (!fs.existsSync(file)) fail(`${pkg}: missing ${relative}`);
    verifyBinary(fs.readFileSync(file), format, machine, libc, pkg);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    verifyPlatformTree(process.argv[2] ? path.resolve(process.argv[2]) : NPM_ROOT);
    console.log("platform-binaries: all seven package binaries match their OS, CPU, and libc contracts");
  } catch (error) {
    console.error(`platform-binaries: ${error.message}`);
    process.exitCode = 1;
  }
}
