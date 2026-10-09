# Local OCR Tool Design

**Status:** Proposed — pending user review

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

- English uses the upstream `robertknight/ocrs` Rust library.
- CJK uses the `kent-tokyo/ocrs-cjk` Rust library with its documented PP-OCRv5
  detection and recognition models and the model's character dictionary.
- Each engine is initialized lazily and cached for reuse. Inference runs on a
  blocking worker so CPU-heavy OCR does not block the async runtime.
- Model weights are downloaded lazily on first use, cached under Code Home's
  `models/ocr` directory, and verified against pinned revisions and SHA-256
  values before being accepted. Weights are not bundled in the repository or
  executable.
- The CJK model files are from the Apache-2.0 model repository. The English
  model page identifies the weights as CC-BY-SA-4.0; the tool must preserve
  attribution and must not redistribute those weights.
- Images are read and processed locally. Network access is used only to fetch
  a missing model file; image bytes and recognized text are not uploaded by
  the OCR implementation.

The two selected crates currently resolve different `rten` versions. The
implementation will keep the user-requested engines separate and account for
the resulting additional dependency/build footprint rather than silently
substituting another engine.

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
- `robertknight/ocrs` model-card metadata and its model-license discussion.
