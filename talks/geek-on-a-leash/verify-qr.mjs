#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import jsQR from "jsqr";
import { PNG } from "pngjs";

const [qrFile, receiptFile] = process.argv.slice(2);
if (!qrFile || !receiptFile) {
  console.error("Usage: node verify-qr.mjs QR.png private-publish-receipt.json");
  process.exit(2);
}

const png = PNG.sync.read(await fs.readFile(qrFile));
const decoded = jsQR(new Uint8ClampedArray(png.data), png.width, png.height);
if (!decoded?.data) throw new Error("QR image did not decode");

const receipt = JSON.parse(await fs.readFile(receiptFile, "utf8"));
if (decoded.data !== receipt.signed_url) throw new Error("QR payload does not match the signed PDF URL");

const response = await fetch(decoded.data, { headers: { "User-Agent": "geek-on-a-leash-qr-verifier/1.0" } });
if (!response.ok) throw new Error(`QR target returned HTTP ${response.status}`);
const bytes = new Uint8Array(await response.arrayBuffer());
const sha256 = crypto.createHash("sha256").update(bytes).digest("hex");
const expected = receipt.objects?.pdf?.sha256;
if (sha256 !== expected) throw new Error("QR target PDF hash does not match the published receipt");

console.log(JSON.stringify({
  decoded: true,
  target_status: response.status,
  content_type: response.headers.get("content-type")?.split(";")[0] || null,
  bytes: bytes.length,
  sha256,
  expires_at: receipt.expires_at,
}, null, 2));
