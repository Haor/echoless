// 崩溃取证日志:落盘到 <brand_data_root>/logs/echoless-<UTC时间戳>.log。
//
// 动机(2026-07-09 黑屏 RCA):此前全部诊断信息都是易失的 —— CLI stderr 只转发
// 成 echoless://log 事件、前端错误只进 DevTools console,app 一关什么都不剩,
// 用户报障只能现场连调试器。本模块给「用户把日志文件发过来」这条路。
//
// 约束(防膨胀):
//   - 每次启动一个新文件(定位「哪次运行出的事」天然清晰);
//   - 启动时清理:mtime 超过 KEEP_DAYS 的删掉,再按新旧保留 KEEP_FILES 个;
//   - 单文件 MAX_BYTES 封顶,超限写一行截断标记后本次不再落盘
//     (防 stderr 风暴刷爆磁盘;事件转发不受影响,UI 照常)。
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const KEEP_DAYS: u64 = 7;
const KEEP_FILES: usize = 20;
const MAX_BYTES: u64 = 8 * 1024 * 1024;

struct Sink {
    file: File,
    written: u64,
    truncated: bool,
}

static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 进程启动时调用一次。失败静默(日志是辅助设施,绝不影响主功能)。
pub(crate) fn init(app_version: &str) {
    let (base, _) = echoless_paths::brand_data_root();
    let dir = base.join("logs");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    prune(&dir);
    let path = dir.join(format!("echoless-{}.log", file_stamp()));
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = LOG_DIR.set(dir);
    let _ = SINK.set(Mutex::new(Some(Sink {
        file,
        written: 0,
        truncated: false,
    })));
    log(
        "info",
        "app",
        &format!(
            "echoless {app_version} started · os={} arch={}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );
}

/// 追加一行:`2026-07-09 10:15:30Z [level] source: msg`。
pub(crate) fn log(level: &str, source: &str, msg: &str) {
    let Some(lock) = SINK.get() else { return };
    let Ok(mut guard) = lock.lock() else { return };
    let Some(sink) = guard.as_mut() else { return };
    if sink.truncated {
        return;
    }
    let line = format!("{} [{level}] {source}: {msg}\n", line_stamp());
    let bytes = line.len() as u64;
    if sink.written + bytes > MAX_BYTES {
        let _ = sink.file.write_all(
            format!("{} [warn] log: size cap reached, truncated\n", line_stamp()).as_bytes(),
        );
        sink.truncated = true;
        return;
    }
    if sink.file.write_all(line.as_bytes()).is_ok() {
        sink.written += bytes;
    }
}

/// 清理:先删超龄,再按 mtime 新→旧只保留 KEEP_FILES-1 个(本次启动还要新建一个)。
fn prune(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut files: Vec<(PathBuf, SystemTime)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?;
            if !(name.starts_with("echoless-") && name.ends_with(".log")) {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((p, mtime))
        })
        .collect();
    files.retain(|(p, mtime)| {
        let expired = now
            .duration_since(*mtime)
            .map(|age| age.as_secs() > KEEP_DAYS * 86_400)
            .unwrap_or(false);
        if expired {
            let _ = fs::remove_file(p);
        }
        !expired
    });
    files.sort_by_key(|entry| std::cmp::Reverse(entry.1)); // 新在前
    for (p, _) in files.into_iter().skip(KEEP_FILES.saturating_sub(1)) {
        let _ = fs::remove_file(p);
    }
}

/// 前端错误汇入同一文件(ErrorBoundary / window.onerror / unhandledrejection)。
#[tauri::command]
pub(crate) fn frontend_log(level: String, message: String) {
    let lv = match level.as_str() {
        "error" | "warn" | "info" => level.as_str(),
        _ => "info",
    };
    // 单条截断:前端可能塞整个组件栈,给 8 KB 足够定位且不失控。
    let msg: String = message.chars().take(8192).collect();
    log(lv, "frontend", &msg);
}

// ---- UTC 时间戳(不引 chrono:civil-from-days 算法,够用) ----

fn now_parts() -> (u64, u64, u64, u64, u64, u64) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let rem = secs % 86_400;
    (y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn line_stamp() -> String {
    let (y, mo, d, h, mi, s) = now_parts();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}Z")
}

fn file_stamp() -> String {
    let (y, mo, d, h, mi, s) = now_parts();
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Howard Hinnant 的 civil_from_days(公历,proleptic)。
fn civil_from_days(z: i64) -> (u64, u64, u64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024-01-01
        assert_eq!(civil_from_days(20_643), (2026, 7, 9)); // 本 RCA 当天
    }
}
