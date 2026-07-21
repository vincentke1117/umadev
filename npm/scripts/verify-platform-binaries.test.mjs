import assert from "node:assert/strict";
import test from "node:test";

import { verifyBinary } from "./verify-platform-binaries.mjs";

function elf(machine, loader = "") {
  const out = Buffer.alloc(160);
  Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1]).copy(out);
  out.writeUInt16LE(machine, 18);
  out.write(loader, 64, "latin1");
  return out;
}

function macho(cpu) {
  const out = Buffer.alloc(64);
  out.writeUInt32LE(0xfeedfacf, 0);
  out.writeUInt32LE(cpu, 4);
  return out;
}

function pe(machine) {
  const out = Buffer.alloc(128);
  out.write("MZ", 0, "ascii");
  out.writeUInt32LE(64, 0x3c);
  out.write("PE\0\0", 64, "ascii");
  out.writeUInt16LE(machine, 68);
  return out;
}

test("accepts each supported executable family", () => {
  assert.doesNotThrow(() => verifyBinary(elf(62, "/lib64/ld-linux-x86-64.so.2"), "elf", 62, "glibc"));
  assert.doesNotThrow(() => verifyBinary(elf(183), "elf", 183, "musl"));
  assert.doesNotThrow(() => verifyBinary(macho(0x0100000c), "macho", 0x0100000c));
  assert.doesNotThrow(() => verifyBinary(pe(0x8664), "pe", 0x8664));
});

test("rejects OS, CPU, and libc package swaps", () => {
  assert.throws(() => verifyBinary(macho(0x0100000c), "elf", 183, "glibc"), /ELF/);
  assert.throws(() => verifyBinary(elf(183), "elf", 62, "musl"), /machine/);
  assert.throws(() => verifyBinary(elf(62, "/lib64/ld-linux-x86-64.so.2"), "elf", 62, "musl"), /GNU dynamic loader/);
  assert.throws(() => verifyBinary(elf(62), "elf", 62, "glibc"), /no GNU dynamic loader/);
  assert.throws(() => verifyBinary(pe(0xaa64), "pe", 0x8664), /machine/);
});
