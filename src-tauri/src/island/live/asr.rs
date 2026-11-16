//! 日语离线语音识别：sherpa-onnx NeMo CTC（Parakeet 日语 0.6B INT8）。
//!
//! 参考项目用 Python `sherpa_onnx.OfflineRecognizer.from_nemo_ctc(model, tokens,
//! num_threads)`；Rust 侧对应 `OfflineRecognizerConfig.model_config.nemo_ctc`
//! + `tokens` + `num_threads`（sherpa-onnx 2.x 的安全封装，模型类型由非空路径
//! 自动推断）。
//!
//! 模型来自 `csukuangfj/sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8`，
//! 参考项目落在 `<models_dir>/parakeet-ja-0.6b-int8/`：
//!
//! ```text
//! model.int8.onnx   （约 625MB）
//! tokens.txt
//! ```
//!
//! 识别器内部有原生状态，用 `Mutex` 串行化（草稿 / 整句两次识别共用一份
//! 模型实例时不会并发进 C API）。

use std::path::Path;
use std::sync::Mutex;

use sherpa_onnx::{OfflineNemoEncDecCtcModelConfig, OfflineRecognizer, OfflineRecognizerConfig};

use super::{file_nonempty, ASR_MODEL_FILE, ASR_TOKENS_FILE, TARGET_RATE};

/// 日语识别引擎：持有唯一一份 sherpa-onnx `OfflineRecognizer`。
pub(crate) struct AsrEngine {
    inner: Mutex<OfflineRecognizer>,
}

impl AsrEngine {
    /// 加载 `<model_dir>/model.int8.onnx` + `tokens.txt`。
    ///
    /// `num_threads` 是 sherpa 的推理线程数（设置文档默认 6，范围 1~16）。
    pub fn create(model_dir: &Path, num_threads: usize) -> Result<Self, String> {
        let model = model_dir.join(ASR_MODEL_FILE);
        let tokens = model_dir.join(ASR_TOKENS_FILE);
        if !file_nonempty(&model) || !file_nonempty(&tokens) {
            return Err(format!(
                "日语识别模型缺失或不完整（{}），请在实时字幕设置里重新下载",
                model_dir.display()
            ));
        }
        let threads = num_threads.clamp(1, 16) as i32;
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.nemo_ctc = OfflineNemoEncDecCtcModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        config.model_config.num_threads = threads;
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| "日语识别模型加载失败（请重新下载模型）".to_string())?;
        Ok(Self {
            inner: Mutex::new(recognizer),
        })
    }

    /// 识别一段 16k 单声道 f32，返回去掉首尾空白的日文文本。
    ///
    /// 空音频直接返回空串（不建 stream）；解码失败/无结果也返回空串，由上层
    /// 当作「这句没听清」处理。
    pub fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let recognizer = self
            .inner
            .lock()
            .map_err(|_| "日语识别引擎状态异常，请重启实时字幕".to_string())?;
        let stream = recognizer.create_stream();
        stream.accept_waveform(TARGET_RATE as i32, samples);
        recognizer.decode(&stream);
        Ok(stream
            .get_result()
            .map(|result| result.text.trim().to_string())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn create_reports_missing_model_dir() {
        let dir = super::super::temp_dir("asr-missing");
        let err = AsrEngine::create(&dir, 2)
            .err()
            .expect("空目录必须报错");
        assert!(
            err.contains("日语识别模型"),
            "错误文案应说明模型问题，实际：{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_rejects_empty_model_files() {
        let root = super::super::temp_dir("asr-empty");
        let dir = super::super::asr_model_dir(&root);
        fs::create_dir_all(&dir).expect("创建模型目录");
        fs::write(dir.join(ASR_MODEL_FILE), b"").expect("写空模型");
        fs::write(dir.join(ASR_TOKENS_FILE), b"").expect("写空词表");
        let err = AsrEngine::create(&dir, 2).err().expect("0 字节模型必须报错");
        assert!(
            err.contains("日语识别模型"),
            "错误文案应说明模型问题，实际：{err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// 真实模型冒烟测试（默认 ignore，不拖慢常规单测）。
    ///
    /// 模型来源：`csukuangfj/sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8`，
    /// 需要把 `model.int8.onnx` + `tokens.txt` 放到 `<models_dir>/parakeet-ja-0.6b-int8/`
    /// （`models_dir` 取实时字幕设置，默认 `~/.agenthud/models`），或用环境变量
    /// `AGENTHUD_LIVE_MODEL_DIR` 直接指定该目录。
    ///
    /// 运行：`cargo test --lib island::live::asr -- --ignored --nocapture`
    #[test]
    #[ignore = "需要本地 625MB 日语模型"]
    fn transcribe_real_model_smoke() {
        let dir: PathBuf = match std::env::var("AGENTHUD_LIVE_MODEL_DIR") {
            Ok(value) => PathBuf::from(value),
            Err(_) => match super::super::models_root() {
                Ok(root) => super::super::asr_model_dir(&root),
                Err(err) => {
                    eprintln!("跳过：{err}");
                    return;
                }
            },
        };
        if !dir.join(ASR_MODEL_FILE).is_file() || !dir.join(ASR_TOKENS_FILE).is_file() {
            eprintln!("跳过：{} 下没有完整模型", dir.display());
            return;
        }
        let engine = AsrEngine::create(&dir, 2).expect("加载真实日语模型");
        let silence = vec![0.0_f32; TARGET_RATE as usize];
        let text = engine.transcribe(&silence).expect("识别静音");
        eprintln!("静音识别结果：{text:?}");
        assert!(engine.transcribe(&[]).expect("空音频").is_empty());
    }
}
