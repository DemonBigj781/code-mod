# Local OCR Tool Design

**Status:** Approved — implementation complete

## Goal

Add local OCR to the Code toolset so an agent can read text in a local card
image without sending the image to a hosted OCR service.

## User-facing tools

- `ocr.read.ckj` runs the CJK OCR engine requested by the user.
- `ocr.read.eng` runs the English OCR engine requested by the user.
- Both functions accept a required `path` to a local image. Relative paths are
  resolved against the session working directory, following `image_view`.
- Both return recognized text with line breaks. Invalid paths, unsupported or
  corrupt images, unavailable models, and download failures return actionable
  tool errors rather than panicking.

The spelling `ckj` is intentional for the exposed tool name because it is the
name supplied by the user; the implementation backend is the CJK OCR project.

## Engine and model choices

- English uses the upstream `robertknight/ocrs` Rust library, pinned to
  version 0.12.2 so it shares RTen 0.24 with the selected CJK release and
  remains compatible with this repository's Rust 1.93 toolchain.
- CJK uses the `kent-tokyo/ocrs-cjk` Rust library with its documented PP-OCRv5
  detection and recognition models and the model's character dictionary.
- Each engine is initialized lazily and cached for reuse. Inference runs on a
  blocking worker so CPU-heavy OCR does not block the async runtime.
- Model weights are downloaded lazily on first use, cached under Code Home's
  `models/ocr` directory, and verified against pinned revisions and SHA-256
  values before being accepted. Weights are not bundled in the repository or
  executable.
- The CJK models and dictionary are pinned to Hugging Face repository revision
  `06c3603ca8002e22e1f41d47c1aae0a251b4d940`. The English model files are
  fetched from upstream S3 URLs and pinned by SHA-256.
- The CJK model files are from the Apache-2.0 model repository. The English
  model-weight repository has no formal `LICENSE` file. Its maintainer says he
  considers the weights CC-BY-SA-4.0 based on training data and calls that a
  "safe view." The implementation will treat the weights conservatively as
  CC-BY-SA-4.0, preserve attribution, and not redistribute them; the license
  ambiguity remains an explicit risk.
- Images are read and processed locally. Network access is used only to fetch
  a missing model file; image bytes and recognized text are not uploaded by
  the OCR implementation.

Both selected crates use RTen 0.24. The English crate is pinned to the latest
release compatible with that RTen version and the repository's Rust 1.93
toolchain; each requested language still uses its own OCR engine and model
set.

## Alternatives considered

- **Require local model paths:** avoids automatic network access, but requires
  users to source and configure several model files manually. Not selected
  because lazy downloads were chosen.
- **Use the CJK engine for both languages:** could reduce backend integration
  work, but would discard the requested upstream English engine. Not selected.
- **Invoke an external OCR CLI:** avoids linking the runtimes into Code, but
  requires a separately installed binary and complicates cross-platform use.
  Not selected.

## Boundaries and failure behavior

- The handler MUST use the same local-path resolution convention as
  `image_view`, require a regular image file, and reject unsupported image
  formats before inference.
- Model downloads MUST use pinned upstream revisions, verify SHA-256, write
  atomically, and leave no usable partial model on interruption.
- If models are cached, OCR MUST work without network access. If a model is
  missing and its download fails, the tool MUST return a clear error including
  the affected engine and cache location.
- OCR MUST NOT be implemented by invoking a separately installed CLI.
- The initial result is recognized text only; bounding boxes, confidence
  scores, translation, card identification, and image upload are out of scope.

## Scenarios

1. **First CJK read** — GIVEN a valid local image and a missing CJK cache, WHEN
   `ocr.read.ckj` is called, THEN the pinned CJK models are fetched, verified,
   cached, and used locally to return recognized text.
2. **Offline cached read** — GIVEN the requested engine's verified models are
   cached and the network is unavailable, WHEN its OCR tool is called, THEN
   local inference succeeds without a network request.
3. **Download failure** — GIVEN a required model is not cached and retrieval
   fails or its checksum differs, WHEN OCR is called, THEN the tool returns a
   clear failure and does not load or retain the invalid file.
4. **Bad image input** — GIVEN a missing, non-regular, unsupported, or corrupt
   path, WHEN either OCR tool is called, THEN it returns an actionable error
   without starting model inference.
5. **English read** — GIVEN a supported local image and the English model,
   WHEN `ocr.read.eng` is called, THEN it returns recognized English text and
   does not invoke the CJK engine.

## Integration and verification

The implementation follows the existing built-in tool pattern: schema in
`openai_tools`, handler registration in the OpenAI tool registry and router,
and a dedicated handler module. Tests cover exact tool names and schemas,
local file validation, model-cache integrity/atomicity, offline cache use, and
engine dispatch. A local inference smoke test will verify each backend with a
small fixture image and the pinned models; normal unit tests will not fetch
large model weights or depend on network access.

## Source evidence

- `kent-tokyo/ocrs-cjk` README and Cargo manifests.
- `robertknight/ocrs` README and Cargo manifests.
- `marsena/paddleocr-onnx-models` model card and file metadata.
- `robertknight/ocrs-models` license discussion (issue 34) and maintainer
  comment.

## Pinned model checksums

- English `text-detection.onnx`:
  SHA-256 `a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe`.
- English `text-recognition.onnx`:
  SHA-256 `86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5`.
- CJK `PP-OCRv5_server_det_infer.onnx`:
  SHA-256 `127edf0182bb3d218ad59476377b02ca90296cfb4cc85df55042d671a3e53aeb`.
- CJK `PP-OCRv5_server_rec_infer.onnx`:
  SHA-256 `13d0dda27d63dc0f4938af48df2c55b33f3c989a0bd5eacb8410e30f1735f644`.
- CJK `PP-OCRv5_server_rec_infer.yml`:
  SHA-256 `2c719dba044c4e2228aef8ff92f5f575394d75d24c16de096a33b7cfd902f66d`.
