use crate::codex::Session;
use crate::codex::ToolCallCtx;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::events::execute_custom_tool;
use crate::tools::handlers::{tool_error, tool_output};
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;
use async_trait::async_trait;
use futures_util::StreamExt;
use image::{ImageFormat, RgbImage};
use ocrs::{OcrEngine as EnglishEngine, OcrEngineParams as EnglishEngineParams};
use ocrs_cjk::{OcrEngine as CjkEngine, OcrEngineParams as CjkEngineParams};
use rten::Model;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::OnceCell;

const ENGLISH_DETECTION: ModelFile = ModelFile {
    filename: "text-detection.onnx",
    url: Cow::Borrowed("https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx"),
    sha256: "a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe",
};
const ENGLISH_RECOGNITION: ModelFile = ModelFile {
    filename: "text-recognition.onnx",
    url: Cow::Borrowed("https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx"),
    sha256: "86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5",
};

const CJK_DETECTION: ModelFile = ModelFile {
    filename: "PP-OCRv5_server_det_infer.onnx",
    url: Cow::Borrowed(
        "https://huggingface.co/marsena/paddleocr-onnx-models/resolve/06c3603ca8002e22e1f41d47c1aae0a251b4d940/PP-OCRv5_server_det_infer.onnx",
    ),
    sha256: "127edf0182bb3d218ad59476377b02ca90296cfb4cc85df55042d671a3e53aeb",
};
const CJK_RECOGNITION: ModelFile = ModelFile {
    filename: "PP-OCRv5_server_rec_infer.onnx",
    url: Cow::Borrowed(
        "https://huggingface.co/marsena/paddleocr-onnx-models/resolve/06c3603ca8002e22e1f41d47c1aae0a251b4d940/PP-OCRv5_server_rec_infer.onnx",
    ),
    sha256: "13d0dda27d63dc0f4938af48df2c55b33f3c989a0bd5eacb8410e30f1735f644",
};
const CJK_DICTIONARY: ModelFile = ModelFile {
    filename: "PP-OCRv5_server_rec_infer.yml",
    url: Cow::Borrowed(
        "https://huggingface.co/marsena/paddleocr-onnx-models/resolve/06c3603ca8002e22e1f41d47c1aae0a251b4d940/PP-OCRv5_server_rec_infer.yml",
    ),
    sha256: "2c719dba044c4e2228aef8ff92f5f575394d75d24c16de096a33b7cfd902f66d",
};

#[derive(Clone)]
struct ModelFile {
    filename: &'static str,
    url: Cow<'static, str>,
    sha256: &'static str,
}

type EngineCell<T> = Arc<OnceCell<Arc<T>>>;

static ENGLISH_ENGINES: LazyLock<Mutex<HashMap<PathBuf, EngineCell<EnglishEngine>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static CJK_ENGINES: LazyLock<Mutex<HashMap<PathBuf, EngineCell<CjkEngine>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) struct OcrToolHandler;

#[async_trait]
impl ToolHandler for OcrToolHandler {
    fn scheduling_hints(&self) -> crate::tools::registry::ToolSchedulingHints {
        crate::tools::registry::ToolSchedulingHints::pure_parallel()
    }

    async fn handle(
        &self,
        sess: &Session,
        _turn_diff_tracker: &mut TurnDiffTracker,
        inv: ToolInvocation,
    ) -> code_protocol::models::ResponseInputItem {
        let ToolPayload::Function { arguments } = inv.payload else {
            return tool_error(inv.ctx.call_id, "OCR expects function-call arguments");
        };

        let language = match inv.tool_name.as_str() {
            crate::openai_tools::OCR_CJK_TOOL_NAME => OcrLanguage::Cjk,
            crate::openai_tools::OCR_ENGLISH_TOOL_NAME => OcrLanguage::English,
            _ => return tool_error(inv.ctx.call_id, "Unknown OCR engine"),
        };
        handle_ocr(sess, &inv.ctx, arguments, language).await
    }
}

#[derive(Clone, Copy)]
enum OcrLanguage {
    Cjk,
    English,
}

impl OcrLanguage {
    fn label(self) -> &'static str {
        match self {
            Self::Cjk => "CJK",
            Self::English => "English",
        }
    }

    fn cache_dir(self, code_home: &Path) -> PathBuf {
        code_home.join("models").join("ocr").join(match self {
            Self::Cjk => "cjk",
            Self::English => "english",
        })
    }
}

#[derive(Deserialize)]
struct OcrParams {
    path: String,
}

async fn handle_ocr(
    sess: &Session,
    ctx: &ToolCallCtx,
    arguments: String,
    language: OcrLanguage,
) -> code_protocol::models::ResponseInputItem {
    let params: OcrParams = match serde_json::from_str(&arguments) {
        Ok(params) => params,
        Err(error) => {
            return tool_error(
                ctx.call_id.clone(),
                format!("Invalid OCR arguments: {error}"),
            );
        }
    };
    let mut params_for_event = serde_json::from_str::<serde_json::Value>(&arguments).ok();
    execute_custom_tool(
        sess,
        ctx,
        match language {
            OcrLanguage::Cjk => crate::openai_tools::OCR_CJK_TOOL_NAME,
            OcrLanguage::English => crate::openai_tools::OCR_ENGLISH_TOOL_NAME,
        }
        .to_owned(),
        params_for_event.take(),
        move || async move {
            let call_id = ctx.call_id.clone();
            let image_path = match resolve_image_path(&params.path, sess.get_cwd(), language).await
            {
                Ok(path) => path,
                Err(error) => return tool_error(call_id, error),
            };

            let image = match tokio::task::spawn_blocking(move || load_rgb_image(&image_path)).await
            {
                Ok(Ok(image)) => image,
                Ok(Err(error)) => return tool_error(call_id, error),
                Err(error) => {
                    return tool_error(call_id, format!("OCR image worker failed: {error}"));
                }
            };

            let code_home = sess.code_home().to_path_buf();
            let text = match recognize_image(language, code_home, image).await {
                Ok(text) => text,
                Err(error) => return tool_error(call_id, error),
            };
            tool_output(call_id, text)
        },
    )
    .await
}

async fn resolve_image_path(
    raw_path: &str,
    cwd: &Path,
    language: OcrLanguage,
) -> Result<PathBuf, String> {
    let path = raw_path.trim();
    if path.is_empty() {
        return Err("OCR requires a non-empty image path".to_owned());
    }

    let mut resolved = PathBuf::from(path);
    if resolved.is_relative() {
        resolved = cwd.join(resolved);
    }
    let resolved = tokio::fs::canonicalize(&resolved).await.map_err(|error| {
        format!(
            "{} OCR could not resolve image path: {error}",
            language.label()
        )
    })?;
    let metadata = tokio::fs::metadata(&resolved).await.map_err(|error| {
        format!(
            "{} OCR could not inspect image path: {error}",
            language.label()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "{} OCR requires a regular image file",
            language.label()
        ));
    }
    Ok(resolved)
}

fn load_rgb_image(path: &Path) -> Result<RgbImage, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("OCR could not read {}: {error}", path.display()))?;
    let format = image::guess_format(&bytes)
        .map_err(|error| format!("OCR could not identify image format: {error}"))?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err(format!(
            "OCR supports PNG and JPEG images; {} is unsupported",
            format
                .extensions_str()
                .first()
                .copied()
                .unwrap_or("unknown")
        ));
    }
    image::load_from_memory_with_format(&bytes, format)
        .map(|image| image.into_rgb8())
        .map_err(|error| format!("OCR could not decode image: {error}"))
}

async fn recognize_image(
    language: OcrLanguage,
    code_home: PathBuf,
    image: RgbImage,
) -> Result<String, String> {
    let cache_dir = language.cache_dir(&code_home);
    match language {
        OcrLanguage::Cjk => {
            let engine = get_cjk_engine(code_home).await.map_err(|error| {
                format!(
                    "CJK OCR initialization failed (model cache {}): {error:#}",
                    cache_dir.display()
                )
            })?;
            tokio::task::spawn_blocking(move || recognize_cjk(&engine, image))
                .await
                .map_err(|error| format!("CJK OCR worker failed: {error}"))?
                .map_err(|error| format!("CJK OCR inference failed: {error:#}"))
        }
        OcrLanguage::English => {
            let engine = get_english_engine(code_home).await.map_err(|error| {
                format!(
                    "English OCR initialization failed (model cache {}): {error:#}",
                    cache_dir.display()
                )
            })?;
            tokio::task::spawn_blocking(move || recognize_english(&engine, image))
                .await
                .map_err(|error| format!("English OCR worker failed: {error}"))?
                .map_err(|error| format!("English OCR inference failed: {error:#}"))
        }
    }
}

async fn get_english_engine(code_home: PathBuf) -> anyhow::Result<Arc<EnglishEngine>> {
    let cache_dir = OcrLanguage::English.cache_dir(&code_home);
    let cell = engine_cell(&ENGLISH_ENGINES, cache_dir.clone())?;
    let engine = cell
        .get_or_try_init(|| async move {
            let client =
                crate::default_client::create_client(crate::default_client::DEFAULT_ORIGINATOR);
            let detection = ensure_model_file(&client, &cache_dir, &ENGLISH_DETECTION).await?;
            let recognition = ensure_model_file(&client, &cache_dir, &ENGLISH_RECOGNITION).await?;
            tokio::task::spawn_blocking(move || {
                let detection_model = Model::load_file(detection)?;
                let recognition_model = Model::load_file(recognition)?;
                EnglishEngine::new(EnglishEngineParams {
                    detection_model: Some(detection_model),
                    recognition_model: Some(recognition_model),
                    ..Default::default()
                })
                .map(Arc::new)
            })
            .await
            .map_err(|error| anyhow::anyhow!("English OCR model worker failed: {error}"))?
        })
        .await?;
    Ok(Arc::clone(engine))
}

async fn get_cjk_engine(code_home: PathBuf) -> anyhow::Result<Arc<CjkEngine>> {
    let cache_dir = OcrLanguage::Cjk.cache_dir(&code_home);
    let cell = engine_cell(&CJK_ENGINES, cache_dir.clone())?;
    let engine = cell
        .get_or_try_init(|| async move {
            let client =
                crate::default_client::create_client(crate::default_client::DEFAULT_ORIGINATOR);
            let detection = ensure_model_file(&client, &cache_dir, &CJK_DETECTION).await?;
            let recognition = ensure_model_file(&client, &cache_dir, &CJK_RECOGNITION).await?;
            let dictionary = ensure_model_file(&client, &cache_dir, &CJK_DICTIONARY).await?;
            tokio::task::spawn_blocking(move || {
                let detection_model = Model::load_file(detection)?;
                let recognition_model = Model::load_file(recognition)?;
                let alphabet = read_cjk_alphabet(&dictionary)?;
                CjkEngine::new(CjkEngineParams {
                    detection_model: Some(detection_model),
                    recognition_model: Some(recognition_model),
                    alphabet: Some(alphabet.into_iter().collect()),
                    ..Default::default()
                })
                .map(Arc::new)
            })
            .await
            .map_err(|error| anyhow::anyhow!("CJK OCR model worker failed: {error}"))?
        })
        .await?;
    Ok(Arc::clone(engine))
}

fn engine_cell<T>(
    engines: &LazyLock<Mutex<HashMap<PathBuf, EngineCell<T>>>>,
    cache_dir: PathBuf,
) -> anyhow::Result<EngineCell<T>> {
    let mut engines = engines
        .lock()
        .map_err(|_| anyhow::anyhow!("OCR engine cache lock was poisoned"))?;
    Ok(Arc::clone(
        engines
            .entry(cache_dir)
            .or_insert_with(|| Arc::new(OnceCell::new())),
    ))
}

async fn ensure_model_file(
    client: &reqwest::Client,
    cache_dir: &Path,
    model: &ModelFile,
) -> anyhow::Result<PathBuf> {
    tokio::fs::create_dir_all(cache_dir).await?;
    let destination = cache_dir.join(model.filename);
    if verify_model_file(destination.clone(), model.sha256).await? {
        return Ok(destination);
    }
    match tokio::fs::remove_file(&destination).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let temporary = cache_dir.join(format!(".{}.{}.tmp", model.filename, uuid::Uuid::new_v4()));
    let result = async {
        let response = client
            .get(model.url.as_ref())
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);

        let actual = format!("{:x}", hasher.finalize());
        if actual != model.sha256 {
            anyhow::bail!(
                "SHA-256 mismatch for {} (expected {}, got {})",
                model.filename,
                model.sha256,
                actual
            );
        }
        if let Err(rename_error) = tokio::fs::rename(&temporary, &destination).await {
            if verify_model_file(destination.clone(), model.sha256).await? {
                tokio::fs::remove_file(&temporary).await?;
            } else {
                return Err(rename_error.into());
            }
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;

    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    Ok(destination)
}

async fn verify_model_file(path: PathBuf, expected_sha256: &'static str) -> anyhow::Result<bool> {
    tokio::task::spawn_blocking(move || match digest_file(&path) {
        Ok(actual) => Ok(actual == expected_sha256),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(anyhow::Error::from(error)),
    })
    .await
    .map_err(|error| anyhow::anyhow!("model cache verification worker failed: {error}"))?
}

fn digest_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn read_cjk_alphabet(path: &Path) -> anyhow::Result<Vec<char>> {
    let contents = std::fs::read_to_string(path)?;
    let document: serde_yaml::Value = serde_yaml::from_str(&contents)?;
    let post_process = document
        .get("PostProcess")
        .ok_or_else(|| anyhow::anyhow!("CJK dictionary is missing PostProcess"))?;
    let dictionary = post_process
        .get("character_dict")
        .and_then(serde_yaml::Value::as_sequence)
        .ok_or_else(|| anyhow::anyhow!("CJK dictionary is missing character_dict"))?;
    let mut alphabet = dictionary
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .and_then(|text| text.chars().next())
                .ok_or_else(|| anyhow::anyhow!("CJK character_dict contains an empty entry"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if post_process
        .get("use_space_char")
        .and_then(serde_yaml::Value::as_bool)
        .unwrap_or(true)
    {
        alphabet.push(' ');
    }
    Ok(alphabet)
}

fn recognize_english(engine: &EnglishEngine, image: RgbImage) -> anyhow::Result<String> {
    use ocrs::ImageSource;

    let input =
        engine.prepare_input(ImageSource::from_bytes(image.as_raw(), image.dimensions())?)?;
    let words = engine.detect_words(&input)?;
    let lines = engine.find_text_lines(&input, &words);
    let recognized = engine.recognize_text(&input, &lines)?;
    Ok(recognized
        .into_iter()
        .flatten()
        .map(|line| line.to_string())
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

fn recognize_cjk(engine: &CjkEngine, image: RgbImage) -> anyhow::Result<String> {
    use ocrs_cjk::ImageSource;

    let input =
        engine.prepare_input(ImageSource::from_bytes(image.as_raw(), image.dimensions())?)?;
    let words = engine.detect_words(&input)?;
    let lines = engine.find_text_lines(&input, &words);
    let recognized = engine.recognize_text(&input, &lines)?;
    Ok(recognized
        .into_iter()
        .flatten()
        .map(|line| line.to_string())
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn model_cache_is_verified_and_reused_offline() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello"))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().expect("temp model cache");
        let model = ModelFile {
            filename: "fixture.onnx",
            url: Cow::Owned(format!("{}/model", server.uri())),
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        };
        let client = reqwest::Client::new();

        let path = ensure_model_file(&client, dir.path(), &model)
            .await
            .expect("download verified model");
        assert_eq!(std::fs::read(&path).expect("cached model"), b"hello");
        ensure_model_file(&client, dir.path(), &model)
            .await
            .expect("reuse cached model without another request");
    }

    #[tokio::test]
    async fn checksum_mismatch_never_leaves_a_model_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"untrusted"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().expect("temp model cache");
        let model = ModelFile {
            filename: "bad.onnx",
            url: Cow::Owned(format!("{}/model", server.uri())),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        };

        let result = ensure_model_file(&reqwest::Client::new(), dir.path(), &model).await;
        assert!(result.is_err());
        assert!(!dir.path().join(model.filename).exists());
        assert_eq!(
            std::fs::read_dir(dir.path())
                .expect("cache directory")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn relative_image_paths_resolve_against_session_cwd() {
        let dir = tempfile::tempdir().expect("temp image directory");
        let image_path = dir.path().join("image.png");
        ImageBuffer::from_pixel(2, 2, Rgb([255u8, 255, 255]))
            .save(&image_path)
            .expect("write valid image");

        let resolved = resolve_image_path("image.png", dir.path(), OcrLanguage::English)
            .await
            .expect("resolve relative image path");
        assert_eq!(resolved, image_path.canonicalize().expect("canonical path"));
        assert!(
            resolve_image_path(" ", dir.path(), OcrLanguage::English)
                .await
                .is_err()
        );
        assert!(
            resolve_image_path("missing.png", dir.path(), OcrLanguage::Cjk)
                .await
                .is_err()
        );
        assert!(
            resolve_image_path(".", dir.path(), OcrLanguage::Cjk)
                .await
                .is_err()
        );
    }

    #[test]
    fn rejects_corrupt_and_unsupported_images_before_inference() {
        let dir = tempfile::tempdir().expect("temp image directory");
        let corrupt = dir.path().join("corrupt.png");
        std::fs::write(&corrupt, b"not a png").expect("write corrupt image");
        assert!(load_rgb_image(&corrupt).is_err());

        let gif = dir.path().join("unsupported.gif");
        std::fs::write(&gif, b"GIF89a").expect("write unsupported image");
        assert!(load_rgb_image(&gif).is_err());

        let valid = dir.path().join("valid.png");
        ImageBuffer::from_pixel(2, 2, Rgb([255u8, 255, 255]))
            .save(&valid)
            .expect("write valid image");
        assert_eq!(
            load_rgb_image(&valid)
                .expect("decode valid PNG")
                .dimensions(),
            (2, 2)
        );
    }

    #[test]
    fn extracts_the_pp_ocr_dictionary_and_optional_space() {
        let dir = tempfile::tempdir().expect("temp dictionary directory");
        let path = dir.path().join("dictionary.yml");
        std::fs::write(
            &path,
            "PostProcess:\n  character_dict: [\"日\", \"本\"]\n  use_space_char: true\n",
        )
        .expect("write dictionary");
        assert_eq!(
            read_cjk_alphabet(&path).expect("read alphabet"),
            vec!['日', '本', ' ']
        );
    }
}
