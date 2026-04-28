//! TensorRT 推論ワーカーの IPC プロトコル型定義。
//!
//! メインプロセス ↔ 子ワーカープロセス間の通信:
//! - **コマンド/レスポンス**: stdin/stdout に行単位 JSON
//! - **テンソルデータ**: 共有メモリ (名前付き、CreateFileMappingW)
//!
//! コマンドや小さなメタ情報だけを JSON でやり取りし、入出力テンソルの
//! 巨大なバイト列は共有メモリにマップする。stdout パイプの 64 KB 帯域では
//! タイル単位 (12+ MB/output) の推論を捌けないため。
//!
//! プロトコル例:
//! ```text
//! 親 → 子: '{"cmd":"load_model","kind":"realesrgan_anime6b"}\n'
//! 子 → 親: '{"ok":true,"elapsed_ms":1234}\n'
//!
//! 親 → 子: '{"cmd":"infer","kind":"realesrgan_anime6b",
//!            "input_shm":"miv_trt_in_1234_0","input_bytes":786432,
//!            "input_shape":[1,3,256,256],
//!            "output_shm":"miv_trt_out_1234_0","output_capacity":12582912}\n'
//! 子 → 親: '{"ok":true,"elapsed_ms":15,"output_shape":[1,3,1024,1024]}\n'
//!
//! 親 → 子: '{"cmd":"shutdown"}\n'
//! (子はセッションを破棄して exit)
//! ```

use serde::{Deserialize, Serialize};

/// 親 → 子 へのコマンド。stdin に行単位 JSON で書く。
///
/// `tag = "cmd"` の externally-tagged enum。`#[serde(rename_all = "snake_case")]`
/// で variant 名を Python 風に。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum WorkerCmd {
    /// 指定 ModelKind のセッションをワーカー側でロード (TRT engine cache HIT
    /// なら数秒、cold compile なら 30 秒〜数分)。
    LoadModel { kind: String },

    /// 指定モデルで推論を 1 回実行する。実データは共有メモリ経由。
    ///
    /// 親が `input_shm` に `input_bytes` バイト書き込んでからこのコマンドを送る。
    /// 子は推論完了後、`output_shm` の先頭から書き込み、`Resp::Ok` で
    /// `output_shape` を返す。
    Infer {
        kind: String,
        input_shm: String,
        input_bytes: usize,
        /// NCHW 形状 (例: [1, 3, 256, 256])
        input_shape: Vec<i64>,
        output_shm: String,
        /// `output_shm` の最大容量バイト数。子が実際に書くのは output_shape 分のみ。
        output_capacity: usize,
    },

    /// 子プロセスを終了させる。応答は `Resp::Ok` で、その後子は exit する。
    Shutdown,
}

/// Infer 推論の詳細タイミング内訳 (ワーカー内部視点、調査用)。
///
/// 各値は ms。worker → parent に Resp で返される。Phase 3 のオーバーヘッド
/// 分析用、本番でも軽く出すコストなので常時付与する。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerInferBreakdown {
    /// 入力共有メモリの read + Vec<f32> 構築
    pub read_input_ms: f64,
    /// ndarray::Array4 + ort::value::Tensor::from_array 構築
    pub tensor_build_ms: f64,
    /// session.run() 純粋時間 (= Direct TRT との比較対象)
    pub session_run_ms: f64,
    /// try_extract_tensor + 出力共有メモリへの write
    pub extract_and_write_ms: f64,
}

/// 子 → 親 のレスポンス。stdout に行単位 JSON で書く。
///
/// `ok: bool` で成功/失敗を表現する untagged 風 (`tag` 無し)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerResp {
    pub ok: bool,
    /// コマンド実行に要した時間 (ms)。Infer の場合は内部処理全体 (with_session 含む)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// `Infer` 成功時の出力テンソル shape (NCHW)。他のコマンドでは None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_shape: Option<Vec<i64>>,
    /// `Infer` の詳細タイミング (調査用)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakdown: Option<WorkerInferBreakdown>,
    /// `ok=false` のときのエラーメッセージ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WorkerResp {
    pub fn ok_simple(elapsed_ms: u64) -> Self {
        Self {
            ok: true,
            elapsed_ms: Some(elapsed_ms),
            output_shape: None,
            breakdown: None,
            error: None,
        }
    }

    pub fn ok_infer(
        elapsed_ms: u64,
        output_shape: Vec<i64>,
        breakdown: WorkerInferBreakdown,
    ) -> Self {
        Self {
            ok: true,
            elapsed_ms: Some(elapsed_ms),
            output_shape: Some(output_shape),
            breakdown: Some(breakdown),
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            elapsed_ms: None,
            output_shape: None,
            breakdown: None,
            error: Some(msg.into()),
        }
    }
}

/// メインから子プロセスを起動するときに渡す引数。
pub const TRT_INFER_WORKER_ARG: &str = "--tensorrt-infer-worker";

/// 共有メモリ名のプリフィックス。`miv_trt_<role>_<pid>_<seq>` 形式。
///
/// PID を含めることで複数 mIV インスタンスが同時起動しても衝突しない。
/// `<role>` は "in" / "out"。`<seq>` は親プロセスの単調増加カウンタで、
/// 異なる入力サイズ用に複数の shm を持てるようにする。
pub const SHM_NAME_PREFIX: &str = "miv_trt";

/// 共有メモリ名を生成する。
pub fn shm_name(role: &str, pid: u32, seq: u32) -> String {
    format!("{SHM_NAME_PREFIX}_{role}_{pid}_{seq}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_roundtrip_load_model() {
        let c = WorkerCmd::LoadModel {
            kind: "realesrgan_anime6b".to_string(),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, r#"{"cmd":"load_model","kind":"realesrgan_anime6b"}"#);
        let back: WorkerCmd = serde_json::from_str(&s).unwrap();
        match back {
            WorkerCmd::LoadModel { kind } => assert_eq!(kind, "realesrgan_anime6b"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn cmd_roundtrip_infer() {
        let c = WorkerCmd::Infer {
            kind: "realesrgan_anime6b".to_string(),
            input_shm: "miv_trt_in_1234_0".to_string(),
            input_bytes: 786432,
            input_shape: vec![1, 3, 256, 256],
            output_shm: "miv_trt_out_1234_0".to_string(),
            output_capacity: 12582912,
        };
        let s = serde_json::to_string(&c).unwrap();
        let back: WorkerCmd = serde_json::from_str(&s).unwrap();
        match back {
            WorkerCmd::Infer {
                kind, input_bytes, ..
            } => {
                assert_eq!(kind, "realesrgan_anime6b");
                assert_eq!(input_bytes, 786432);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn cmd_roundtrip_shutdown() {
        let c = WorkerCmd::Shutdown;
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, r#"{"cmd":"shutdown"}"#);
    }

    #[test]
    fn resp_ok_simple_serialize() {
        let r = WorkerResp::ok_simple(1234);
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""ok":true"#));
        assert!(s.contains(r#""elapsed_ms":1234"#));
        assert!(!s.contains("output_shape"));
        assert!(!s.contains("error"));
    }

    #[test]
    fn resp_err_serialize() {
        let r = WorkerResp::err("model not found");
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""ok":false"#));
        assert!(s.contains(r#""error":"model not found""#));
    }

    #[test]
    fn shm_name_format() {
        assert_eq!(shm_name("in", 1234, 0), "miv_trt_in_1234_0");
        assert_eq!(shm_name("out", 5678, 42), "miv_trt_out_5678_42");
    }
}
