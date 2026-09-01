# Geek on a Leash

An editable 47-slide, one-hour Rust Tuesdays talk about Leash: a Rust boundary between agent intent and physical motion.

The deck spends approximately 52 minutes in pinned Leash source and CUDA kernels, 5 minutes on a bounded live UGV demonstration, and 3 minutes on questions. Thirty-seven slides show source directly: newtypes, checked domain values, typestate, `PhantomData`, compile-fail doctests, traits and associated types, static and dynamic dispatch, ownership across threads, `Drop`, bounded generics, atomics and memory ordering, `Option` ownership operations, Serde wire contracts, error conversion, ROS2 frame types, checked CUDA artifacts, kernel indexing and masking, device-buffer ownership, the isolated CUDA `unsafe` launch, reduction atomics, predictive updates, end-to-end break-even measurements, shadow parity, and CPU fallback. Qualia appears once as the slower mission/ontology layer; it is not presented as part of Leash.

## Artifacts

- `output/geek-on-a-leash.pptx` — editable PowerPoint with 47 speaker-note sections
- `output/geek-on-a-leash.pdf` — presentation PDF
- `output/speaker-notes.md` — portable copy of the embedded notes
- `output/fallback-demo.mp4` — 24-second, explicitly labelled recorded fallback
- `output/geek-on-a-leash-qr.svg` and `.png` — direct-PDF QR
- `output/manifest.json` — public artifact hashes and Railway object metadata after publication

## Build

```sh
npm install
TALK_QR_PNG="$PWD/output/geek-on-a-leash-qr.png" \
TALK_QR_EXPIRES='Direct PDF QR • valid through Sep 8, 2026 12:59 PM ET' \
node deck.mjs output/geek-on-a-leash.pptx
soffice --headless --convert-to pdf --outdir output output/geek-on-a-leash.pptx
node verify-deck.mjs output/geek-on-a-leash.pptx
node verify-qr.mjs output/geek-on-a-leash-qr.png output/private-publish-receipt.json
```

`output/private-publish-receipt.json` contains the signed URL and is intentionally ignored by Git. Railway direct object links are presigned and the generated QR expires on the date printed in the slides.

## Demo preflight

Observation-only preflight never actuates the robot:

```sh
node demo-preflight.mjs --base-url http://PINKIE:8000
```

Motion readiness adds the operator-token gate but still does not actuate:

```sh
node demo-preflight.mjs --base-url http://PINKIE:8000 --motion
```

The live sequence is deliberately bounded:

1. Pass read-only observation gates.
2. Obtain explicit human approval and an active operator token.
3. Issue one pulse at no more than `0.10` normalized drive for no more than `500 ms`.
4. Verify zero and show the evidence record.
5. If any required gate is red, use `fallback-demo.mp4`.

The talk does not claim whole-house autonomy, active mapping, or visual-odometry lock. At the final build-time check on 2026-09-01, Pinkie's previously verified `10.0.0.34:8000` endpoint was offline, and the preflight correctly returned `BLOCKED`.

## Publish

Publishing is constrained to `geek-on-a-leash/v1/` in the existing Railway bucket. `prepare` uploads the provisional PDF, creates and verifies the direct-link QR, and writes a private receipt. `finalize` overwrites the same PDF key with the QR-bearing final file, uploads the PPTX/video/manifest, and verifies the final PDF through the original signed URL.

Run `node publish.mjs --help` for the required explicit project, environment, and bucket IDs.
