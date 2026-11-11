//! Commands for the site owner, run on the server itself (never over HTTP):
//!
//! ```text
//! outhouse-server evidence-files <report-id> [--any]
//! outhouse-server evidence-cat <report-id> <storage-key>
//! outhouse-server export-evidence <report-id> [--operator NAME]
//! outhouse-server evidence-handover-log <report-id> <package-sha256> <file-count> <package-name> [--operator NAME]
//! outhouse-server evidence-package <report-id> [--any] [--operator NAME] [--reference TEXT]
//! ```
//!
//! The safety console hands evidence to police from the app (a one-time
//! download link; api/police.rs). These are the fallback over SSH:
//! deploy/evidence-handover.sh runs `evidence-package` (through `docker
//! compose exec`), which writes the very same sealed package the app makes
//! (handover.rs) to stdout and logs it. The older commands stay for anyone
//! scripting them. They work on quarantined files too: those are never
//! served over HTTP, and this only ever streams a file's bytes, unchanged,
//! to whoever runs the command on the server. Output goes to stdout;
//! messages and logs go to stderr.

use std::io::Write;

use anyhow::{bail, Context};
use serde_json::json;

use crate::api::safety;
use crate::handover;
use crate::state::AppState;

pub const COMMANDS: &[&str] = &[
    "evidence-files",
    "evidence-cat",
    "export-evidence",
    "evidence-handover-log",
    "evidence-package",
];

/// Exit codes the script understands.
pub const EXIT_NOT_FOUND: i32 = 4;
pub const EXIT_NOT_ELIGIBLE: i32 = 3;
pub const EXIT_USAGE: i32 = 2;

fn usage() -> i32 {
    eprintln!(
        "usage:\n  outhouse-server evidence-files <report-id> [--any]\n  outhouse-server evidence-cat <report-id> <storage-key>\n  outhouse-server export-evidence <report-id> [--operator NAME]\n  outhouse-server evidence-handover-log <report-id> <package-sha256> <file-count> <package-name> [--operator NAME]\n  outhouse-server evidence-package <report-id> [--any] [--operator NAME] [--reference TEXT]"
    );
    EXIT_USAGE
}

struct Args {
    rest: Vec<String>,
    /// `--operator NAME`: who ran it on the server.
    operator: Option<String>,
    /// `--reference TEXT`: officer name, police reference or case number.
    reference: Option<String>,
    any: bool,
}

fn clean_arg(raw: Option<String>, max: usize) -> Option<String> {
    raw.map(|o| {
        o.chars()
            .filter(|c| !c.is_control())
            .take(max)
            .collect::<String>()
    })
    .filter(|o| !o.trim().is_empty())
}

fn split_args(args: &[String]) -> Args {
    let mut rest = Vec::new();
    let mut operator = None;
    let mut reference = None;
    let mut any = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--operator" => operator = it.next().cloned(),
            "--reference" => reference = it.next().cloned(),
            "--any" => any = true,
            _ => rest.push(a.clone()),
        }
    }
    Args {
        rest,
        operator: clean_arg(operator, 64),
        reference: clean_arg(reference, 200),
        any,
    }
}

fn parse_id(raw: Option<&String>) -> Option<i64> {
    raw.and_then(|r| r.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
}

use crate::handover::safe_key;

/// Over SSH, only child-safety reports or ones with quarantined files,
/// unless `--any` (the app packs any report).
fn eligible(category: &str, files: &[handover::ReportFile], any: bool, id: i64) -> bool {
    if category == safety::CHILD_SAFETY || files.iter().any(|f| f.quarantined) || any {
        return true;
    }
    eprintln!(
        "Report {id} isn't a child-safety report and has no quarantined files. Add --any to package it anyway."
    );
    false
}

pub async fn run(st: AppState, command: &str, args: &[String]) -> anyhow::Result<i32> {
    let Args {
        rest,
        operator,
        reference,
        any,
    } = split_args(args);
    let Some(id) = parse_id(rest.first()) else {
        return Ok(usage());
    };
    let report: Option<(String,)> = sqlx::query_as("SELECT category FROM reports WHERE id = $1")
        .bind(id)
        .fetch_optional(&st.db)
        .await?;
    let Some((category,)) = report else {
        eprintln!("There's no report {id}.");
        return Ok(EXIT_NOT_FOUND);
    };
    let operator_label = operator.clone().unwrap_or_else(|| "unknown".to_string());
    match command {
        "evidence-files" => {
            let rows = handover::report_files(&st, id)
                .await
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            if !eligible(&category, &rows, any, id) {
                return Ok(EXIT_NOT_ELIGIBLE);
            }
            let mut out = std::io::stdout().lock();
            // key, sha256 (as recorded when reported), size, on disk, quarantined
            for f in rows {
                if !safe_key(&f.storage_key) {
                    continue;
                }
                let on_disk = st.config.upload_dir.join(&f.storage_key).is_file();
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{}",
                    f.storage_key,
                    f.sha256.unwrap_or_else(|| "-".into()),
                    f.size,
                    if on_disk { "yes" } else { "no" },
                    if f.quarantined { "yes" } else { "no" }
                )?;
            }
            Ok(0)
        }
        "evidence-package" => {
            let rows = handover::report_files(&st, id)
                .await
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            if !eligible(&category, &rows, any, id) {
                return Ok(EXIT_NOT_ELIGIBLE);
            }
            let req = handover::Request {
                report_id: id,
                operator: operator_label.clone(),
                reference: reference.unwrap_or_default(),
                route: handover::Route::Console,
                generated_by: json!({}),
            };
            let package = handover::build(&st, &req, &handover::packages_dir(&st))
                .await
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            // Straight to stdout, then off the server's disk.
            let copied = (|| -> std::io::Result<()> {
                let mut file = std::fs::File::open(&package.path)?;
                let mut out = std::io::stdout().lock();
                std::io::copy(&mut file, &mut out)?;
                out.flush()
            })();
            let _ = std::fs::remove_file(&package.path);
            copied.context("writing the package")?;
            safety::log_action(
                &st,
                None,
                "handover",
                Some(id),
                None,
                handover::log_details(&req, &package, ""),
            )
            .await;
            eprintln!(
                "Packed {} file(s) for report {id}: {} ({} bytes)\nSHA-256 {}",
                package.file_count,
                package.file_name(),
                package.size,
                package.sha256
            );
            if !package.missing.is_empty() {
                eprintln!(
                    "Note: {} file(s) were no longer on the server (listed in HANDOVER.txt).",
                    package.missing.len()
                );
            }
            if !package.mismatched.is_empty() {
                eprintln!(
                    "WARNING: {} file(s) don't match their recorded SHA-256 (see HANDOVER.txt).",
                    package.mismatched.len()
                );
            }
            eprintln!("Written to the safety log.");
            Ok(0)
        }
        "evidence-cat" => {
            let Some(key) = rest.get(1).filter(|k| safe_key(k)) else {
                return Ok(usage());
            };
            let belongs: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM report_files WHERE report_id = $1 AND storage_key = $2)",
            )
            .bind(id)
            .bind(key)
            .fetch_one(&st.db)
            .await?;
            if !belongs {
                eprintln!("That file isn't part of report {id}.");
                return Ok(EXIT_NOT_FOUND);
            }
            let path = st.config.upload_dir.join(key);
            let mut file = std::fs::File::open(&path)
                .with_context(|| format!("{key} isn't on the server any more"))?;
            let mut out = std::io::stdout().lock();
            std::io::copy(&mut file, &mut out)?;
            out.flush()?;
            Ok(0)
        }
        "export-evidence" => {
            let package = safety::evidence_package(
                &st,
                id,
                json!({ "operator": operator_label, "via": "the server console (outhouse-server export-evidence)" }),
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            let mut out = std::io::stdout().lock();
            serde_json::to_writer_pretty(&mut out, &package)?;
            writeln!(out)?;
            out.flush()?;
            safety::log_action(
                &st,
                None,
                "export",
                Some(id),
                None,
                format!(
                    "Exported the evidence package on the server (no files), by {operator_label}"
                ),
            )
            .await;
            Ok(0)
        }
        "evidence-handover-log" => {
            let (Some(hash), Some(count), Some(name)) = (rest.get(1), rest.get(2), rest.get(3))
            else {
                return Ok(usage());
            };
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("the package SHA-256 should be 64 hex characters");
            }
            let count: u32 = count.parse().context("the file count should be a number")?;
            let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
            safety::log_action(
                &st,
                None,
                "handover",
                Some(id),
                None,
                format!(
                    "Evidence packaged for police on the server by {operator_label}: {count} file(s) and the evidence package in {name} (SHA-256 {})",
                    hash.to_lowercase()
                ),
            )
            .await;
            eprintln!("Written to the safety log.");
            Ok(0)
        }
        _ => Ok(usage()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_and_keys() {
        let args: Vec<String> = ["12", "--operator", "kim\u{7}", "--any"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = split_args(&args);
        assert_eq!(a.rest, vec!["12".to_string()]);
        assert_eq!(a.operator.as_deref(), Some("kim"));
        assert!(a.any && a.reference.is_none());
        assert_eq!(parse_id(a.rest.first()), Some(12));
        let r: Vec<String> = ["3", "--reference", "Det. Smith #42"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(split_args(&r).reference.as_deref(), Some("Det. Smith #42"));
        assert_eq!(parse_id(Some(&"-3".to_string())), None);
        assert!(safe_key("abc123DEF"));
        assert!(!safe_key("../etc/passwd"));
        assert!(!safe_key(""));
    }
}
