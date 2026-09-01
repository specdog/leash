#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import {
  GetObjectCommand,
  HeadObjectCommand,
  PutObjectCommand,
  S3Client,
} from "@aws-sdk/client-s3";
import { getSignedUrl } from "@aws-sdk/s3-request-presigner";
import QRCode from "qrcode";

const SESSION = process.env.RAILWAY_AGENT_SESSION || "railway-skill-geek-on-a-leash-publish";

function parseArgs(argv) {
  const options = {
    phase: "prepare",
    expiresDays: 90,
    prefix: "geek-on-a-leash/v1",
    privateReceipt: null,
    manifest: null,
    qrSvg: null,
    qrPng: null,
    pptx: null,
    video: null,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const next = () => {
      const value = argv[++index];
      if (!value) throw new Error(`${argument} requires a value`);
      return value;
    };
    if (argument === "--phase") options.phase = next();
    else if (argument === "--project-id") options.projectId = next();
    else if (argument === "--environment-id") options.environmentId = next();
    else if (argument === "--bucket-id") options.bucketId = next();
    else if (argument === "--pdf") options.pdf = next();
    else if (argument === "--pptx") options.pptx = next();
    else if (argument === "--video") options.video = next();
    else if (argument === "--prefix") options.prefix = next().replace(/^\/+|\/+$/g, "");
    else if (argument === "--expires-days") options.expiresDays = Number(next());
    else if (argument === "--private-receipt") options.privateReceipt = next();
    else if (argument === "--manifest") options.manifest = next();
    else if (argument === "--qr-svg") options.qrSvg = next();
    else if (argument === "--qr-png") options.qrPng = next();
    else if (argument === "--help") {
      console.log(`Usage: node publish.mjs --phase prepare|finalize [options]

Required: --project-id --environment-id --bucket-id --pdf --private-receipt
Prepare also requires: --qr-svg --qr-png
Finalize also requires: --pptx --video --manifest

All writes are constrained to --prefix (default: geek-on-a-leash/v1).`);
      process.exit(0);
    } else throw new Error(`Unknown argument: ${argument}`);
  }
  for (const key of ["projectId", "environmentId", "bucketId", "pdf", "privateReceipt"]) {
    if (!options[key]) throw new Error(`Missing --${key.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`);
  }
  if (!["prepare", "finalize"].includes(options.phase)) throw new Error("--phase must be prepare or finalize");
  if (options.phase === "prepare" && (!options.qrSvg || !options.qrPng)) throw new Error("prepare requires --qr-svg and --qr-png");
  if (options.phase === "finalize" && (!options.pptx || !options.video || !options.manifest)) throw new Error("finalize requires --pptx, --video, and --manifest");
  if (!Number.isFinite(options.expiresDays) || options.expiresDays <= 0) throw new Error("--expires-days must be positive");
  return options;
}

function railwayCredentials({ projectId, environmentId, bucketId }) {
  const query = `query TalkBucketCredentials($projectId: String!, $environmentId: String!, $bucketId: String!) {
    bucketS3Credentials(projectId: $projectId, environmentId: $environmentId, bucketId: $bucketId) {
      accessKeyId secretAccessKey bucketName endpoint region urlStyle createdAt
    }
  }`;
  const result = spawnSync(
    "railway",
    [
      "api",
      query,
      "--raw-var", `projectId=${projectId}`,
      "--raw-var", `environmentId=${environmentId}`,
      "--raw-var", `bucketId=${bucketId}`,
      "--compact",
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        RAILWAY_CALLER: "skill:use-railway@1.3.7",
        RAILWAY_AGENT_SESSION: SESSION,
      },
      maxBuffer: 1024 * 1024,
    },
  );
  if (result.status !== 0) throw new Error(`Railway credential lookup failed: ${result.stderr.trim() || `exit ${result.status}`}`);
  const parsed = JSON.parse(result.stdout);
  const credentials = parsed?.data?.bucketS3Credentials;
  if (!Array.isArray(credentials) || credentials.length === 0) throw new Error("Railway returned no bucket credentials");
  return credentials.sort((left, right) => Date.parse(right.createdAt) - Date.parse(left.createdAt))[0];
}

async function sha256(file) {
  const hash = crypto.createHash("sha256");
  hash.update(await fs.readFile(file));
  return hash.digest("hex");
}

async function upload(client, bucketName, key, file, contentType, disposition = "attachment") {
  const body = await fs.readFile(file);
  await client.send(new PutObjectCommand({
    Bucket: bucketName,
    Key: key,
    Body: body,
    ContentType: contentType,
    ContentDisposition: `${disposition}; filename="${path.basename(file)}"`,
    CacheControl: "private, max-age=3600",
  }));
  const head = await client.send(new HeadObjectCommand({ Bucket: bucketName, Key: key }));
  if (Number(head.ContentLength) !== body.length) throw new Error(`Uploaded size mismatch for ${key}`);
  return { key, bytes: body.length, content_type: contentType, sha256: await sha256(file) };
}

async function verifySignedPdf(url, expectedHash) {
  const response = await fetch(url, { headers: { "User-Agent": "geek-on-a-leash-publisher/1.0" } });
  if (!response.ok) throw new Error(`Signed PDF returned HTTP ${response.status}`);
  const contentType = response.headers.get("content-type")?.split(";")[0];
  if (contentType !== "application/pdf") throw new Error(`Signed PDF returned ${contentType || "no content type"}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  const actualHash = crypto.createHash("sha256").update(bytes).digest("hex");
  if (actualHash !== expectedHash) throw new Error("Signed PDF hash does not match the final local PDF");
  return { status: response.status, bytes: bytes.length, content_type: contentType, sha256: actualHash };
}

async function prepare(options, client, bucketName) {
  const pdfKey = `${options.prefix}/geek-on-a-leash.pdf`;
  const pdf = await upload(client, bucketName, pdfKey, options.pdf, "application/pdf", "inline");
  let signedUrl;
  let actualDays;
  let verification;
  const attempts = [...new Set([options.expiresDays, 7])];
  for (const days of attempts) {
    try {
      signedUrl = await getSignedUrl(
        client,
        new GetObjectCommand({
          Bucket: bucketName,
          Key: pdfKey,
          ResponseContentType: "application/pdf",
          ResponseContentDisposition: 'inline; filename="geek-on-a-leash.pdf"',
        }),
        { expiresIn: Math.round(days * 24 * 60 * 60) },
      );
      verification = await verifySignedPdf(signedUrl, pdf.sha256);
      actualDays = days;
      break;
    } catch (error) {
      if (days === attempts.at(-1)) throw error;
    }
  }
  const expiresAt = new Date(Date.now() + actualDays * 24 * 60 * 60 * 1000).toISOString();
  await fs.mkdir(path.dirname(options.qrSvg), { recursive: true });
  await fs.mkdir(path.dirname(options.qrPng), { recursive: true });
  await QRCode.toFile(options.qrSvg, signedUrl, { type: "svg", errorCorrectionLevel: "M", margin: 4, width: 1024, color: { dark: "#0B0B0B", light: "#F7F2E8" } });
  await QRCode.toFile(options.qrPng, signedUrl, { type: "png", errorCorrectionLevel: "M", margin: 4, width: 2048, color: { dark: "#0B0B0B", light: "#F7F2E8" } });
  const privateReceipt = {
    schema_version: "leash.talk-private-publish-receipt.v1",
    phase: "prepare",
    created_at: new Date().toISOString(),
    expires_at: expiresAt,
    signed_url: signedUrl,
    bucket_name: bucketName,
    prefix: options.prefix,
    objects: { pdf },
    verification,
  };
  await fs.mkdir(path.dirname(options.privateReceipt), { recursive: true });
  await fs.writeFile(options.privateReceipt, `${JSON.stringify(privateReceipt, null, 2)}\n`, { mode: 0o600 });
  console.log(JSON.stringify({ phase: "prepare", bucket_name: bucketName, prefix: options.prefix, expires_at: expiresAt, objects: { pdf }, verification, qr_svg: options.qrSvg, qr_png: options.qrPng }, null, 2));
}

async function finalize(options, client, bucketName) {
  const privateReceipt = JSON.parse(await fs.readFile(options.privateReceipt, "utf8"));
  if (privateReceipt.bucket_name !== bucketName || privateReceipt.prefix !== options.prefix) throw new Error("Private receipt scope does not match the selected bucket/prefix");
  const pdfKey = `${options.prefix}/geek-on-a-leash.pdf`;
  const pptxKey = `${options.prefix}/geek-on-a-leash.pptx`;
  const videoKey = `${options.prefix}/fallback-demo.mp4`;
  const manifestKey = `${options.prefix}/manifest.json`;
  const objects = {
    pdf: await upload(client, bucketName, pdfKey, options.pdf, "application/pdf", "inline"),
    pptx: await upload(client, bucketName, pptxKey, options.pptx, "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    video: await upload(client, bucketName, videoKey, options.video, "video/mp4", "inline"),
  };
  const verification = await verifySignedPdf(privateReceipt.signed_url, objects.pdf.sha256);
  const manifest = {
    schema_version: "leash.talk-publish-manifest.v1",
    title: "Geek on a Leash",
    published_at: new Date().toISOString(),
    source_commit: process.env.TALK_SOURCE_COMMIT || null,
    railway: {
      project_id: options.projectId,
      environment_id: options.environmentId,
      bucket_id: options.bucketId,
      bucket_name: bucketName,
      prefix: options.prefix,
      signed_pdf_expires_at: privateReceipt.expires_at,
    },
    objects,
    manifest_key: manifestKey,
    signed_pdf_verification: verification,
  };
  await fs.mkdir(path.dirname(options.manifest), { recursive: true });
  await fs.writeFile(options.manifest, `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o644 });
  const manifestObject = await upload(client, bucketName, manifestKey, options.manifest, "application/json", "inline");
  privateReceipt.phase = "finalize";
  privateReceipt.finalized_at = new Date().toISOString();
  privateReceipt.objects = { ...objects, manifest: manifestObject };
  privateReceipt.verification = verification;
  await fs.writeFile(options.privateReceipt, `${JSON.stringify(privateReceipt, null, 2)}\n`, { mode: 0o600 });
  console.log(JSON.stringify({ phase: "finalize", bucket_name: bucketName, prefix: options.prefix, expires_at: privateReceipt.expires_at, objects: privateReceipt.objects, verification }, null, 2));
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const credentials = railwayCredentials(options);
  const client = new S3Client({
    endpoint: credentials.endpoint,
    region: credentials.region,
    forcePathStyle: ["path", "path-style"].includes(credentials.urlStyle),
    credentials: { accessKeyId: credentials.accessKeyId, secretAccessKey: credentials.secretAccessKey },
  });
  if (options.phase === "prepare") await prepare(options, client, credentials.bucketName);
  else await finalize(options, client, credentials.bucketName);
}

main().catch((error) => {
  console.error(`Publish failed: ${error.message}`);
  process.exitCode = 1;
});
